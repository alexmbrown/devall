# /dev/all — Implementation plan

Feature priorities and the Free/Hosted split live in [`FEATURES.md`](FEATURES.md). This document
fixes the architecture, the on-disk format, the decisions taken, and the milestone order.

## 1. Decisions taken (defaults from the research's "open decisions" table)

| Decision | Choice | Why |
|---|---|---|
| Platforms | **macOS + Windows first-class**, Linux best-effort | WebView2/WKWebView are solid; WebKitGTK has NVIDIA blank-window bugs |
| Shell | Tauri 2.12 (stable; 3.0 is alpha) | Small, Rust core, Yaak/Aptakube/kftray prove it for dev tools |
| Frontend | React 19 + Vite 8 + TypeScript 6 + Zustand + TanStack Query + lucide; Monaco | Same stack as `../wiremock-ui`, so its tokens, shell, UI kit and ~3k lines of tested `src/lib` port directly |
| Core | Cargo workspace of small crates shared by the app, the `devall` CLI and (later) a Docker image | CLI/CI parity from day one; no logic locked inside the UI |
| File format | Own YAML schema, **one entity per file**, ordering in the parent `_folder.yaml` | Avoids Bruno's per-file `seq` merge conflicts; importers cover OpenCollection/Bruno/Postman |
| YAML crate | `serde_yaml_ng` | Maintained fork with the classic `serde_yaml` API (`serde_yaml` is archived) |
| Secrets | References only (`{{secret.NAME}}`); values in the OS keychain (`keyring`), fallback to a git-ignored `.env` | README rule: no secrets in project files, ever |
| HTTP engine | `reqwest` 0.13 (rustls, HTTP/2, cookies, compression) | Mature; hyper-level timings later where needed |
| Mock engine | Native Rust engine that reads/writes **WireMock JSON mappings**; "attach to external WireMock" via admin API | No JRE in the installer; WireMock format keeps files portable |
| Scripting | Sandboxed QuickJS (`rquickjs`, ~210 KiB) with a `pm.*` compatibility layer | Eases Postman migration; tiny |
| Git | Users' own git for writes; `gix` for reads in the git panel | gix still lacks push/rebase |
| Local data | SQLite for history/event store; project files are the source of truth | |
| AI | Local by default; any OpenAI-compatible endpoint; managed signed llama.cpp sidecar later | Privacy; cross-platform (Metal / CPU+Vulkan / CUDA on demand) |
| Accounts | **None** for Group 1. Sign-in only for the hosted relay (Group 2) | Forced login is the #1 backlash trigger in this market |
| Relay | Firebase (Auth, entitlements, webhook inbox) + Cloudflare Workers/Durable Objects tunnel edge, behind a `RelayProvider` trait; relay is open source & self-hostable | Firebase can't hold tunnels; Cloudflare is ~$5/mo flat |
| License | App and CLI free for everyone incl. commercial use; relay open source (AGPL to prevent closed rebrands) | Mission: free what others charge for |

## 2. Repository layout

```
dev-all/
├─ Cargo.toml                    # workspace
├─ crates/
│  ├─ devall-core/               # project model, YAML format, variables, secrets, env resolution
│  ├─ devall-http/               # HTTP execution engine (request → response + timings)
│  ├─ devall-utils/              # typed operation registry: encoders, JWT, hashes, ids, time, formats
│  ├─ devall-mock/               # (M2) WireMock-compatible mock engine
│  ├─ devall-cli/                # `devall` binary: run requests/collections, utils, mocks (CI)
│  └─ …                          # later: devall-proxy, devall-load, devall-data, devall-infra, devall-ai, devall-mcp
├─ apps/desktop/                 # Tauri app
│  ├─ src-tauri/                 # thin command layer over the crates
│  └─ src/                       # React UI (design system ported from ../wiremock-ui)
├─ relay/                        # (M4) Group 2: Cloudflare worker + Firebase functions (open source)
├─ schemas/                      # JSON Schemas for every project file (editor validation)
└─ docs/
```

## 3. Project format v1

A **project** is any folder containing `devall.yaml`. It is usually a git repo (or a sub-folder of one) — or, for a
**multi-repo project**, a folder whose `devall.yaml` lists several repositories (nested under it or anywhere else)
that share its requests, environments, mocks and notes.

```
my-api/
├─ devall.yaml                   # manifest: id, name, global variables
├─ http/                         # requests and folders
│  ├─ _folder.yaml               # optional: display name + child order + shared headers/auth
│  ├─ list-users.req.yaml
│  └─ auth/
│     ├─ _folder.yaml
│     └─ login.req.yaml
├─ environments/
│  ├─ local.env.yaml
│  └─ staging.env.yaml
├─ mocks/                        # (M2) WireMock layout: mappings/*.json + __files/
├─ .env                          # optional local secret fallback — git-ignored
└─ .devall/                      # local-only state (history db, UI state) — git-ignored
```

Rules:

- One entity per file; file name = slug of the entity name; a stable `id` (ULID) lives inside so renames don't break references.
- Ordering lives in the parent `_folder.yaml` (`order: [list-users, auth]`); unknown children sort alphabetically after ordered ones. No per-file sequence numbers.
- Stable key order on write; no timestamps or UI state in tracked files.
- `version: 1` in every file for migrations; JSON Schemas in `schemas/`.
- Creating a project writes/extends `.gitignore` with `.devall/` and `.env*`.

`devall.yaml`
```yaml
version: 1
id: 01JBQ8Z6W4N7Q2F3K9M1T5R8XY
name: My API
variables:            # project-wide, non-secret
  apiVersion: v2
repos:                # optional: multi-repo projects (omitted when empty)
  - name: web                       # unique; files are referenced as repo:web/<path>
    path: frontend/web              # relative to this folder, `/`-separated; may climb out: ../api
    remote: git@github.com:acme/web.git   # offered as "Clone…" when missing on a machine
    group: Frontend                 # free label the app groups repos by
  - name: api
    path: ../api
    group: Backend
```

Repositories (`devall_core::repos`): paths in `devall.yaml` are always relative — never absolute. A machine whose
layout differs keeps its own locations in the project data folder's `repos.json` (`{ "<name>": "<absolute path>" }`,
never committed; "Locate…" in the app, `devall repos locate`). The app also finds the repo containing the project
folder (the single-repo case — no `repos:` needed, exactly as before) and nested repos under it that aren't listed
yet (depth ≤ 3, skipping `node_modules`, `target`, `dist`, hidden and content folders; offered as "Save to
devall.yaml"). Repo-qualified paths `repo:<name>/<path>` work in note/to-do links, OpenAPI spec sources
(`_folder.yaml` `openapi.source`), gRPC `.proto` files/import paths and notebook working directories.

`http/list-users.req.yaml`
```yaml
version: 1
id: 01JBQ90A1B2C3D4E5F6G7H8J9K
name: List users
method: GET
url: "{{baseUrl}}/users"
params:
  - { name: page, value: "1" }
  - { name: debug, value: "true", enabled: false }
headers:
  - { name: Accept, value: application/json }
auth:
  type: bearer                   # none | basic | bearer | apiKey  (oauth2, sigv4, digest… in M1)
  token: "{{secret.API_TOKEN}}"
body:
  type: none                     # none | json | text | xml | form | multipart | graphql | binary
settings:
  timeoutMs: 30000
  followRedirects: true
  verifyTls: true
```

`environments/staging.env.yaml`
```yaml
version: 1
name: Staging
color: amber
protected: true                  # confirm before sending non-GET requests
variables:
  baseUrl: https://staging.example.com
secrets:                         # names only — values live in the OS keychain
  - API_TOKEN
```

Template syntax (resolved in Rust, shared by app + CLI):

| Syntax | Meaning |
|---|---|
| `{{name}}` | environment variable, then project variable |
| `{{secret.NAME}}` | keychain entry `devall/<projectId>/<env>/NAME`, then `.env` `NAME` |
| `{{$env.NAME}}` | OS environment variable |
| `{{$uuid}}` `{{$ulid}}` `{{$timestamp}}` `{{$isoTimestamp}}` `{{$randomInt}}` | dynamic values |

Unresolved variables are reported (UI highlights them; CLI fails unless `--allow-unresolved`).
Every resolved secret value is collected so history, logs, exports and AI prompts can redact it.

## 4. Module status (updated as modules land)

| Module | Crate | Status |
|---|---|---|
| Project format, environments, secrets (keychain, .env, 1Password/Bitwarden/Vault/AWS/GCP/Azure/Doppler/Infisical refs), secret scanner + pre-commit hook | devall-core | ✅ |
| HTTP engine: auth (Basic/Bearer/API key/OAuth 2.0 + PKCE/AWS SigV4/Digest/NTLMv2), mTLS, proxies, cookies, SSE streaming, assertions, response snapshots, captures, runner, JUnit | devall-http | ✅ |
| Response history + compare any two + promote to snapshot test | devall-history | ✅ |
| Utilities (75 ops: regex + saved test cases, diff, jq/JSONPath/JMESPath, JSON→types, cron, x509/TLS/DNS/CIDR, recipes, magic…) | devall-utils | ✅ |
| Fake data generator (relational, seeded, SQL/CSV/JSON/YAML, WireMock template bridge) | devall-data | ✅ |
| Load testing (open/closed models, HDR + CO correction, thresholds, run comparison, CLI exit codes, distributed agents with exact HDR merging) | devall-load | ✅ |
| Notes (Markdown) + personal to-dos + AI context | devall-core | ✅ |
| Source control panel, file watching | app | ✅ |
| Mock server (WireMock-compatible engine + admin API, templating, recording, chaos) + GraphQL (schema auto-mocking, validation), WebSocket message stubs, gRPC (h2c + reflection), MQTT broker | devall-mock | ✅ engine wired |
| WebAssembly plugins (wasmi sandbox; utilities, request hooks, import formats, codegen, auth providers; install + hash-pinned trust) + author SDK | devall-plugins, sdk/rust/devall-plugin | ✅ wired (Plugins page, auth editor, Import / Generate code, `devall plugin`) |
| Importers (Postman, Insomnia, Bruno, OpenCollection, OpenAPI/Swagger, HAR, .http, Hurl, cURL) / exporters (OpenAPI, Postman, .http) / code generation | devall-import | ✅ wired |
| WebSocket / gRPC (reflection + .proto) / MQTT 3.1.1+5 / Socket.IO / GraphQL schema | devall-proto | ✅ wired (tabs, New … menu, GraphQL schema panel) |
| Pre/post scripts (QuickJS sandbox, Postman `pm.*` + chai + CryptoJS, Bruno shims) | devall-script | ✅ wired (Scripts tab on requests + folders, Console tab, runner, `devall run`) |
| Local AI (Ollama/LM Studio/llama.cpp sidecar + BYO cloud, local semantic search) + MCP server (`devall mcp`) | devall-ai, devall-mcp | ✅ wired (AI page incl. Search tab, palette search, assist buttons in response/tests/regex/to-dos) |
| Runnable notebooks (`.ipynb`: shell, PowerShell, JS, Python) | devall-notebook | ✅ wired (`.ipynb` editor, runnable Markdown blocks) |
| SQL (Postgres/MySQL/SQLite/SQL Server) / Redis / Mongo clients + SSH tunnels, read-only by default | devall-db | ✅ wired (Databases page; `connections/*.conn.yaml`, `queries/`) |
| Docker / Kubernetes / GCP (Pub/Sub, Firestore, Storage, Logging) + port-forward → environment override | devall-infra | ✅ wired (Docker & K8s page; forwards override `{{baseUrl}}`; logs → viewer) |
| Log viewer (auto-detected formats, query language, DuckDB SQL, patterns) + project event store | devall-logs | ✅ wired (Logs page; events from sends, runs, load tests, mock traffic) |
| Webhooks (local inbox, verify Stripe/GitHub/Slack/Twilio/Shopify/Standard, re-sign, replay, → request/fixture) + BYO tunnels (cloudflared/ngrok/Tailscale Funnel) | devall-webhooks | ✅ wired (Webhooks page; hosted relay inbox feeds it) |
| Traffic inspector (MITM, rules, breakpoints, capture → stub/HAR/OpenAPI/requests/load test) | devall-proxy | ✅ wired |
| Hosted relay (Group 2): Cloudflare tunnel edge + webhook inbox, Firebase accounts/Stripe, desktop client, self-host guide; custom domains, raw TCP tunnels, hosted mocks, shared snapshots, monitors, cloud load regions | relay/, crates/devall-relay | ✅ wired (Tunnels page, Load page → Cloud regions…; live e2e in relay/e2e; owner must fill `hosted.rs` + deploy — checklist in relay/README.md) |
| Mock server page (port of ../wiremock-ui: stubs, journal, near misses, record, scenarios, files, verify, compare, import/export) | app | ✅ wired (built-in engine + remote WireMock servers; "Mock from response" in the HTTP client; GraphQL / WebSocket / gRPC / MQTT tabs) |

### 4b. Remaining gaps vs FEATURES.md (audit 2026-10-22)

Every module above is wired; these feature-map rows are still partial or missing. Built one batch at a time.

**Batch A — HTTP client and utilities polish (P0–P1)**
- [x] P0 Scratch pad → "Move to project" (copy requests/envs/notes into another project)
- [x] P1 NTLM auth
- [x] P1 Promote a response to a snapshot test; diff any two history entries
- [x] P1 jq / JSONPath filter on live responses; NL → jq; NL → JSONPath button; regex AI uses the test text as samples
- [x] P1 Regex sandbox saved test cases (should / shouldn't match)
- [x] P1 Clipboard auto-detect on focus; inline decode in inspector, webhooks, logs, mocks
- [x] P1 Notes can link to mocks (and other project files); "Summarize notes" button

Batch A notes: `devall_core::transfer` copies items between projects (name clashes → "Name (2)"; secret
*values* are only copied keychain-to-keychain after a confirmation). NTLM runs the NTLMv2 handshake on a
dedicated HTTP/1.1 client with one kept-alive connection (servers that close the connection between the
legs can't authenticate). Snapshots live in `http/__snapshots__/<request path>.snap.json` (hidden from the
request tree); `devall run --update-snapshots` records/accepts them. Saved regexes with their test cases
are project files, `utilities/regex/<slug>.regex.yaml` (schema `regex.schema.json`), so a pattern's
expectations are reviewed and shared like the code that uses it; the Regex tool also keeps them in its
local state.

**Batch B — Mocks in CI, specs and drift (P1–P2)**
- [x] P1 `devall mock serve` CLI + Dockerfile/image + CI workflow
- [x] P1 Load test baseline against the local mock
- [x] P2 CSV / DB-backed mock data sources
- [x] P2 Hurl export; run `.http` / `.hurl` files directly
- [x] P2 OpenAPI watch (re-import on change) + lint
- [x] P2 Spec ↔ mock ↔ traffic drift checks (the validation proxy followed in Batch C)

Batch B notes: `devall mock serve|check|record|health` run the project's `mocks/` headless (text or
JSON-lines request log, chaos presets, SIGTERM-aware); a folder with only `mocks/` works too. The
root `Dockerfile` (Rust builder → `debian:bookworm-slim`, non-root, entrypoint
`devall mock serve --project /project --bind 0.0.0.0 --port 8080`, `HEALTHCHECK` via
`devall mock health`), `.dockerignore` and `.github/workflows/docker.yml` (build + smoke test on
every change, push to GHCR only on `v*` tags) were written without a Docker daemon and are untested;
usage is in `MOCKS_IN_CI.md` and the Mock server's **Run in CI** dialog. Load tests take
`target: mock` (or `devall load run --against-mock` / **Run against mock**): request origins are
rewritten to the project's mock server (started if needed) and the summary carries `mockBaseline`;
comparing a real run with a baseline shows a client-overhead vs server+network breakdown and no
regression verdicts. Data sources are CSV / JSON / SQLite files in `mocks/data/`, read through the
`dataSource` / `dataRows` / `dataCount` helpers or `transformerParameters.dataSource` (rows as
`data` / `row`, optional `notFoundStatus`, read-only `sql` for SQLite) — modelled on WireMock Cloud's
data sources, /dev/all engine only. Hurl export (`export::to_hurl`, + a `.hurl.env` variables file
per environment) and a Hurl codegen target; `devall run file.http|file.hurl` parses with the
importers and runs in memory (`runner::run_requests`, `Project::detached`) with `--env-file` /
`--var` / `--junit`; the Hurl importer now accepts `{{baseUrl}}/…` request lines. The new
`crates/devall-openapi` (pure Rust) has an OpenAPI 3.0/3.1 schema validator (the devall-ai one is
generic JSON Schema; OpenAPI needs nullable, boolean exclusive bounds and read/write-only), a
25-rule linter and drift checks (stubs, inspector captures, request history, HAR) with operation
coverage; CLI `devall openapi lint|drift|sync`, UI in the OpenAPI dialog (request tree folder menu),
the Mock server's **Drift** tab and the Inspector's **Check spec**. A folder links to its spec with
an `openapi: { source, autoSync }` key in `_folder.yaml` (set by the import dialog's "keep in sync"
or the folder menu); re-sync re-imports in place (environments/variables untouched) and lists
requests whose operation disappeared; auto-sync follows `project-changed` for specs inside the
project (external files and URLs re-sync on demand). The validation proxy (an inspector rule that
flags/blocks spec violations live) was deferred and shipped in Batch C (see its notes).

**Batch C — Data and AI (P1–P2)**
- [x] P1 Fake-data schema from a DB schema (table introspection)
- [x] P1 MCP writes: per-call in-app confirmation
- [x] P2 GCP Secret Manager import
- [x] P2 Explain-this-failure gathers mock stub, logs, pod events; cited answer
- [x] P2 Traffic → OpenAPI: LLM naming pass + spec validation
- [x] P2 NL → SQL / Mongo / kubectl, checked with EXPLAIN / dry-run
- [x] P2 llama.cpp sidecar bundled as `externalBin` (signing needs the owner's certificates)
- [x] P2 Validation proxy (deferred from Batch B): inspector rule "Validate against OpenAPI"

Batch C notes: **Fake data from a DB** — `devall_data::fromdb` maps neutral table specs to
entities (type + name heuristics, enum values from Postgres enums / MySQL `enum(…)` / `CHECK (… IN
…)`, sequences/UUIDs for keys, single-column FKs → `ref`, nullable → null rate, reference loops cut at
a nullable column, `table` recorded per entity, seed pinned to 42 so one-by-one seeding stays
consistent); `devall_db::fakeschema` introspects SQL (schema only, never rows) or samples MongoDB
documents (ObjectIds/dates generated as Extended JSON, `userId` → `users._id` refs). Databases page:
table/schema/collection menus and a toolbar button; Fake data page: **New from database…**;
**Seed…** gained "All entities (parents first)" (`db_seed_all`). **MCP approvals** — `devall-mcp`
has a `Confirmer` trait and three write modes (read-only / confirm / allow); protected
environments always need an approval and are refused when nothing can ask. The app's HTTP endpoint
passes an `AppConfirmer` (event `mcp-approval` + pending store + `mcp_approval_answer`; 2-minute
timeout = deny); the global `McpApprovalHost` in `App.tsx` offers Allow once / Allow for this
session (same request, method, environment) / Deny with method, URL, environment, headers and body
(secrets masked). `devall mcp` over stdio keeps the flag behaviour (no UI to ask; protected
environments refused). **Secret Manager** — real projects only (gcloud token), metadata listing;
import stores values in the keychain under the environment scope or adds `gcp-sm://` references
to `secretSources`, and adds the names to `secrets:`; nothing is written to GCP.
**Explain this failure** — `ai_failure_context` gathers the mock stub that answered (or the
closest near misses) via `MockHandle::explain_request`, project events with the same correlation id
or path within ±60 s, and pod status/events + the last 40 log lines when the URL is a running
Kubernetes port-forward (each step time-boxed); the `explainFailure` feature numbers them S1…Sn
(S1 = the exchange) and cites them; gathered context only goes to cloud providers with
`allowRemoteContext` (otherwise listed as "not sent"). The response panel uses it for ≥ 400,
failed tests and transport errors; cited sources open the stub, the event (Logs) or the pod's
namespace. **OpenAPI naming** — `openapiNames` sends an outline (operations, params, body property
names; never example values), validates operationIds/schema names (one repair), applies a patch
(names, summaries, tags, inline bodies → `components/schemas` with `$ref`s, identical shapes shared)
and the dialog shows a diff plus lint before/after; **Check spec** lints whatever is in the editor;
deterministic output stays the default. **NL → SQL/Mongo/kubectl** — `structured::generate_checked`
adds async checks: read-only classification + `EXPLAIN` (never ANALYZE; writes only when the session
is read-write and the question explicitly asks), Mongo `explain`, kubectl parse + object lookups and
server-side dry runs for delete/scale/rollout restart/label/annotate; nothing is executed (kubectl
delete/scale/restart can be run in-app after a confirmation). Schema summaries:
`db_schema_summary` / `db_mongo_fields`. **llama-server bundling** — `externalBin` lives in the
`src-tauri/tauri.bundle.conf.json` overlay because tauri-build copies sidecars and resources on every
build (`cargo check`/`tauri dev` would fail without the file); `scripts/fetch-llama-server.mjs`
verifies against `crates/devall-ai/llama-assets.json` (generated from `catalog.rs`, kept in sync by a
test) and writes `binaries/tauri.libs.conf.json` for the shared libraries; `npm run app:bundle`;
signing in `RELEASING.md` (not run: no download, no certificates here). **Validation proxy** —
`RuleAction::ValidateSpec { spec, mode: flag|block, allHosts }` compiles the spec once
(`devall_openapi::Conformance`, shared with drift checks); requests are checked before forwarding
(block → `422 application/problem+json`, never forwarded), responses after streaming in flag mode
(off the I/O path) or before delivery in block mode (→ `502`); exchanges carry `validation`
(badge in the list, findings in the detail pane).

**Batch D — Hosted extras (Group 2, P2–P3)**
- [x] P2 Custom domains for tunnels
- [x] P3 Raw TCP tunnels
- [x] P3 Publicly hosted mocks
- [x] P3 Shareable request/response snapshots (expiring links)
- [x] P3 Scheduled monitors (Supporter; free has none)
- [x] P3 Cloud load-generator regions (verified targets only, bounded, kill switch)
- [x] `devall tunnel http` / `devall inbox listen` and a live end-to-end run against `wrangler dev`

Batch D notes: every item works on a self-hosted relay for free (static-token `selfhost` plan),
limits are in `relay/worker/src/plans.ts` (new `LimitsPayload` fields, additive in HELLO), the
wire changes are in `relay/PROTOCOL.md` (§2.6 TCP frames `0x40`–`0x43`, §3.1 custom domains, §6
account API) and abuse/cost notes in `relay/README.md`. **Custom domains** — Cloudflare for SaaS
custom hostnames, managed by the Firebase callables `addCustomDomain` / `removeCustomDomain` /
`refreshCustomDomain` (+ `refreshPendingCustomDomains` every 30 min): Supporter only, ≤ 2, must
point at one of the user's reserved subdomains; Firestore `customDomains/{hostname}` is the source
of truth and KV gets `host:<h>` (routing), `subhosts:<sub>` (extra `publicUrls` in HELLO) and
`domains:<uid>` (status + DNS records for `/v1/account`); the Worker routes foreign hosts through
the static `CUSTOM_HOSTS` map (self-host) or KV, and only to a tunnel connected by the mapping
account. Releasing a subdomain with domains attached is refused. Tunnels page → Custom domains
lists status and the CNAME/TXT records with copy buttons. **Raw TCP tunnels** — TCP over
WebSocket (`/connect?kind=tcp` publisher, `/tcp?name=` consumer per connection, end-to-end credit;
the DO only rewrites stream ids and holds pre-upgrade frames); consumers present the owner's
account token or the `dtcp_…` access secret (only its SHA-256 reaches the relay; the desktop keeps
the secret in the keychain with a Rotate button). `devall-relay::tcp` (publisher + consumer),
`devall tunnel tcp --port 5432` / `devall tcp connect <name> --local-port 15432` (CLI relay
selection: `--relay/--token`, `DEVALL_RELAY_URL/TOKEN`, else the app's `relay.json` + keychain),
Tunnels page → TCP tunnels (publish + connect). Supporter-only to publish on the hosted relay;
connecting is free. **Hosted mocks** — `MockDO` per name with a TypeScript matcher/templater for a
documented WireMock subset (`relay/worker/src/hosted-mock-subset.json` is shared with
`devall-relay::hosted_mock`, whose publish check lists every unsupported matcher/helper/response
feature before upload; unsupported matchers never match at the edge); scenarios, journal (last N),
CORS, delays ≤ 10 s, HTML/SVG/JS downgraded to text + CSP sandbox + noindex, per-IP and per-day
limits; URLs `https://<name>.mock.<domain>` and `https://mock.<domain>/<name>/` (the latter needs
no extra certificate and is the default handed out). `devall mock publish|unpublish|published|journal`,
Mock server → **Publish to the web** (plan, warnings, URL), Tunnels page → Hosted mocks (journal,
reset scenarios, unpublish). **Snapshots** — History → **Share…**: the desktop masks the
environment's secret values everywhere and (unless unticked) auth/cookie/API-key headers and
token-like query parameters, then uploads to `ShareDO` (chunked SQLite, an alarm deletes it at
expiry); `https://share.<domain>/<id>` renders a read-only page (nonce'd "Copy as cURL",
`.req.yaml` / JSON / `.sh` downloads, 410 when expired or revoked); Tunnels page → Shared links
lists and revokes. Never anonymous. **Monitors** — `MonitorDO` per monitor; a `*/5` Cron Trigger
lists `mon:<id>` KV keys (interval in the key metadata) and arms due alarms; checks mirror the
project assertion model (snapshots excluded), secrets are uploaded only when ticked by name,
stored AES-GCM-encrypted with an HKDF(`MONITOR_SECRET_KEY`, uid) key and substituted only during a
run; results keep no bodies or secret values; alerts on down/up via webhook JSON or Slack;
downgrades pause monitors beyond the plan. The desktop builds monitor requests from saved requests
(variables of one environment resolved, `{{secret.X}}` kept, basic auth re-encoded at the edge,
OAuth2/SigV4/Digest/NTLM and multipart skipped with a reason) — Tunnels page → Monitors
(sparkline, uptime, run now, pause, edit, results). Monitors are a Supporter feature as the
feature map says (free `monitors: 0`, still overridable with `LIMITS_JSON`); self-hosted relays may
alert plain `http://` listeners; cron retries are deduplicated by slot (scheduled time), not wall
clock. **Cloud load regions** — `LoadDO` per region (`newUniqueId()` + `locationHint`, best-effort,
the colo recorded from a `/cdn-cgi/trace` probe when available) runs an open-model arrival loop with
`fetch()` into a mergeable log-linear histogram; the coordinator (`POST /v1/load/runs`) splits runs
into ≤ 10 s / ≤ 800-request segments (one RPC each; kill switch and stop act between them) and
merges regions into devall-load summary fields. Abuse controls: mandatory ownership verification
(`/.well-known/devall-load-verification.txt` or DNS TXT `_devall-load.<host>`, 30 days, per
`host:port` / per host), no private/loopback/link-local or relay-domain targets on hosted plans,
Supporter ≤ 3 regions · 60 s · 100 req/s · 20 runs/day · one run at a time, `CLOUD_LOAD = "off"` /
`POST /admin/load/disable`, `x-devall-load: <run id>` on every request. `devall-load::cloud` builds
the plan from a normal `Plan` (open-model executors only; fixed requests — dynamic values resolved
once, warned; secrets refused without `--allow-secrets-to-cloud`, like agents) and turns the answer
into a `LoadSummary` (thresholds evaluated client-side, regions as `cloud:<region> (<colo>)` agents)
stored and compared like local runs. CLI `devall load cloud verify|run|status|remove`; Load page →
**Cloud regions…** (verification status + token/file/DNS with copy buttons, region picker capped by
plan, secrets consent). **CLI** — `devall tunnel http --port 3000 [--subdomain] [--json]` (public
URLs incl. custom domains, request log, reconnects, fails fast with the relay's reason when the
first connection is refused) and `devall inbox listen [--project] [--json]` (into the project's
webhook store like the app, ack after storing). **Live e2e** — `relay/e2e/run.mjs` runs the Worker
under `wrangler dev --local` (self-host, static tokens, Host-header routing; the bare loopback host
serves the relay routes) with the real CLI: HTTP/WS/SSE tunnel, TCP tunnel, inbox, mocks, shares,
monitors (cron via the Local Explorer API), custom domain, cloud load, free tier, > 75 s idle
heartbeats, relay restart — 11/11 pass and every process is stopped afterwards. Bugs it found and
fixed: the client lost the relay's JSON refusal (tungstenite keeps only body bytes that arrive with
the response head, so "plan allows 1 tunnel" showed as "HTTP 429") — the handshake now reads the
whole error body; a retryable refusal surfaced only as "timed out waiting for the tunnel" — the
timeout now carries the last error and `devall tunnel http` fails fast; the CLI/app couldn't reach a
local `wrangler dev` relay at all (no Host header control) — loopback hosts now serve the relay
routes; monitors couldn't alert a LAN listener on self-hosted relays. Tests: worker (Miniflare)
unit + integration for TCP bridging, custom-host routing, mocks, shares, monitors with a fake
network and the cron; Firebase custom-domain logic + Cloudflare client with fakes; Rust in-process
mock relay for the TCP publisher/consumer, the API and the callables; publish check and snapshot
redaction unit tests. Nothing was deployed: the owner checklist is in `relay/README.md`.

**Batch E — Local extras (P3)**
- [x] P3 WASM plugin SDK
- [x] P3 gRPC / GraphQL / WebSocket / MQTT mocks
- [x] P3 Distributed load agents (Docker/K8s on the user's infra)
- [x] P3 SQL Server driver
- [x] P3 Local semantic search (embeddings)

Batch E (part 1) notes: **SQL Server** — `DbKind::Sqlserver` (`devall-db/src/sql/mssql.rs`) on
tiberius 0.13 (pure-Rust TDS 7.3–8.0; rustls with aws-lc, no native-tls/OpenSSL; SQL Browser for
named instances). Connections take host/port, `host\INSTANCE` or an `instance` option, `sqlserver://`
/ `mssql://` / `jdbc:sqlserver://…;databaseName=…;encrypt=…;trustServerCertificate=…` URLs, SQL
authentication with `{{secret.X}}` passwords, TLS modes (disable → no TLS, prefer → encrypt without
checks, require → encrypt + trust server certificate, verify-full → validated, optional CA;
`encrypt: strict` = TDS 8.0), `applicationIntent: ReadOnly` (AG secondaries), `statementTimeout`, SSH
tunnels (certificate checked against the real host). Scripts split on `GO` lines (`lex::split_tsql`);
a batch with variables, control flow or a CREATE PROCEDURE/FUNCTION/TRIGGER/VIEW body runs whole,
others split on `;`; one statement can return several result sets (`SqlConn::run_statement_sets`;
only the first pages). Pure reads run under `SET ROWCOUNT offset+limit+1` (exact `more`,
reset after every statement together with `@@TRANCOUNT` for transaction tracking); cancel = TDS
attention on the same connection. SQL Server has no read-only session flag: the T-SQL classifier
(`classify_tsql`) checks every word of a batch (EXEC/dynamic SQL, OPENQUERY, UPDLOCK, NEXT VALUE FOR
… count as writes; `#temp`/`@table` targets don't) and read-only sessions run at READ COMMITTED.
Types: bigint/decimal/money as strings, datetime family as ISO 8601 with `T`, uniqueidentifier
upper-case, varbinary base64. Schema from `sys.*` (schemas, tables, views, columns with display
types, identity, indexes incl. INCLUDE/filters, FKs, CHECKs → best-effort CREATE TABLE); EXPLAIN =
`SET SHOWPLAN_XML ON` (estimated) or `SET STATISTICS XML ON` in a rolled-back transaction (actual),
parsed into the Postgres-shaped plan tree (format `sqlserver`, XML kept as text; the plan view shows
both). Edits use `@P1…` parameters in a transaction; inserts batch ≤ 2 000 parameters / 1 000 rows;
`devall-data` gained `SqlDialect::Sqlserver` (`[ident]`, `N'…'`, `IF OBJECT_ID(…) IS NULL CREATE
TABLE`, T-SQL types) so fake-data export, schema-from-DB (CHECK `N'…'` enums) and seeding work;
NL → SQL asks for T-SQL (`TOP`) and is checked with SHOWPLAN. Live test `DEVALL_TEST_MSSQL_URL`
(skips when unset; not run here — no server); unit tests for the splitter, classifier, type
mapping, URL forms, SHOWPLAN parsing and DDL. Gaps: Windows/Azure AD authentication, PRINT/RAISERROR
messages (tiberius doesn't surface INFO tokens), `GO n` runs once.
**Semantic search** — `devall-ai::embed` (an `Embed` trait; Ollama `/api/embed`, OpenAI-compatible
`/embeddings` for llama-server `--embedding` / LM Studio / vLLM; EmbeddingGemma and Qwen3-Embedding
prompt conventions; `FakeEmbedder` for tests) and `devall-ai::semantic` (chunks: notes by heading
and paragraph, requests by name/method/URL/docs, mock stubs by name/method/URL/response summary,
environments by environment/variable/secret *names*; all redacted; stored in
`.devall/embeddings/index.json` + `vectors.f32`, L2-normalized; incremental by content hash and
embedder id; cancel keeps progress; search = cosine fused with BM25 by reciprocal rank, keyword-only
fallback with a note). `ai.json` → `embeddings: { provider, model }` (a local provider, or the
managed `llama-server --embedding` sidecar started from the catalog; cloud endpoints only with
`allowRemoteContext`). Commands `ai_index_status` / `ai_index_build` (progress channel; cancel via
`ai_cancel`) / `ai_index_clear` / `ai_search` / `ai_embedding_settings` / `ai_embed_sidecar_start|stop`.
UI: AI → **Search** tab (model, index, search) and the command palette (**Search project…** or a `?`
prefix). `ai_ask` (and its preview) fuse the existing keyword rank with the vector hits
(`context::relevant_hybrid`; matching request/mock chunks become their own cited sources). MCP tool
`search_project` (desktop endpoint uses the app's embedder incl. the managed one; `devall mcp` uses
`ai.json`). Tests with the fake embedder (chunking, env values never indexed, incremental rebuild,
model mismatch → keywords, cancel/resume, hybrid Ask ranking, MCP hybrid/keyword).
**Distributed load** — `devall-load::{distributed, agent}`: agents (`devall load agent --listen
0.0.0.0:7777`, token from `DEVALL_AGENT_TOKEN`, optional rustls `--tls-cert/--tls-key`) take an
`AgentPlan` over HTTP+JSON and stream NDJSON snapshots, then their summary and V2-serialized HDR
histograms (`Recorder::export` / `Recorder::merge`: exact merge, tested equal to one recorder); the
coordinator (`devall load run --agents h1:7777,h2:7777 [--agent-weights 2,1] [--agents-token-env]
[--allow-secrets-to-agents] [--agents-ca]`, `dns:<headless service>` expansion; app: Load page →
**Distribute…**) checks every agent first, splits rates/VUs by weight, merges, evaluates thresholds
on the merged recorder and records `agents` in the summary (`load-run.schema.json` regenerated).
Requests using `{{secret.X}}` are refused unless explicitly allowed (only the used values travel);
client certificates and file bodies are refused. Deploy: `Dockerfile` `agent` target (+ CI smoke test
and GHCR publish), `devall load agent k8s` (Secret + Deployment + headless Service), `devall load agent
token|health`; guide in `LOAD_AGENTS.md`. Tests: loopback coordinator + 2 agents vs a local target
(totals, per-agent shares, thresholds on the merge, p50/p90 within tolerance of a single-process
run), bad-token refusal, secret refusal/consent, splitting, k8s YAML.

Batch E (part 2) notes: **WASM plugins** — `crates/devall-plugins` runs plugins in wasmi 1.1 (pure
Rust interpreter: no JIT / executable memory, light build; 2.0.0 was tried and crashed when
optimised on Windows; wasmi is built at opt-level 2 in dev). ABI: exports `memory`, `alloc`,
`devall_call(ptr, len) -> (ptr << 32 | len)` with JSON in/out; imports only `devall.log` and
`devall.host_call` (JSON ops `http.request`, `secrets.get`, `fs.read`, `time.now`,
`random.bytes`), no WASI. Every call gets a fresh instance; fuel is handed out in 10M slices through
resumable out-of-fuel calls so the wall-clock deadline is enforced between slices (1e9 fuel / 5 s
for utilities, importers, codegen; 5e8 / 2 s for hooks and auth), memory is capped at 64 MiB by a
`ResourceLimiter`, and errors are deterministic (`exceeded its instruction budget`, `timed out
after`, `exceeded its memory limit`, `capability denied: …`, which wins over whatever the plugin
made of a denial). `plugin.yaml` (`deny_unknown_fields`) declares capabilities (`http.hosts`
allow-list, `secrets.read` names, `fs.project.read`) and contributions: utility ops (Utilities page
category *Plugins*, command palette, `devall util <plugin>:<op>`), pre-request / post-response hooks
(desktop sends and collection runs and `devall run`, via a `devall_http::plugins::RequestPlugins`
bridge installed at startup; optional per-hook host filter), import formats (`import.detect` /
`import.parse` → an `ImportPlan` with `Format::Plugin`; the Import dialog falls back to them when no
built-in format matches), codegen targets (*Plugins* group in Generate code, `codegen_plugin`) and
auth providers (`Auth::Plugin { plugin, provider, config }` in devall-core, resolved in
`prepare_with`, so load tests and MCP sends sign too). Installed plugins live in
`<app data>/plugins/<id>/` (folder or `.zip`, install pinned to the reviewed SHA-256); project
plugins in `<project>/plugins/<id>/` never run until trusted per machine, pinned to the module's
SHA-256 and the approved capabilities (re-prompt on change; an installed plugin with the same id
shadows the project copy). Approvals, granted secrets and disabled hooks are in
`<app data>/plugins.json`; plugin secrets are keychain entries under `plugin:<id>` and only granted
names are released. UI: SideNav → **Plugins** (install / review & trust / enable / revoke /
uninstall, secrets with grant toggles and Set…, hook toggles, errors) and the auth editor's
*Plugin…* type; CLI `devall plugin list|install|trust|run|uninstall`. The author SDK
`sdk/rust/devall-plugin` (contract types, `Plugin` trait, `export!` macro, host-call wrappers,
host-testable `dispatch`) and `examples/plugins/hmac-signer` (HMAC-SHA256 auth provider + utility)
are outside the workspace; both pass `cargo test` / clippy on the host. The wasm32 target is not
installed here, so the example was not built to wasm; the runtime is tested with hand-written WAT
modules instead (every contribution point, fuel exhaustion, wall-clock timeout, memory cap at growth
and at instantiation, capability denials for HTTP / secrets / files, trust pinning incl. capability
changes and shadowing, zip install, WASI-import and missing-export refusals, the HTTP bridge through
`devall_http::prepare`). Guide: `docs/PLUGINS.md`. Gaps: no component model / WIT, no UI
contributions, hooks don't see multipart bodies, load tests run plugin auth but not hooks.
**Protocol mocks** (all in `crates/devall-mock`, files under the project's `mocks/`, one message
journal `GET|DELETE /__admin/devall/messages?protocol=`, status `GET /__admin/devall/protocols`):
*GraphQL* — own lexer/parser (queries + SDL incl. `extend` and block strings), schema model from SDL
(`mocks/graphql/*.graphql|graphqls|gql`, `<name>/` folders merged) or introspection JSON,
validation with graphql-js messages (fields, leaf selections, known / required arguments, literal
and variable types, fragments: known / unused / cycles / possible spreads, variables defined /
unused / input types / positions, directives, lone anonymous operation; not: overlapping-field
merging), variable coercion, deterministic auto-mocking shaped by the selection set (field-name
heuristics → emails, names, dates, prices…, seeded per response path; `id` arguments flow into `id`
fields; `first` / `limit`… set list lengths; abstract types pick the fragment's type), full
introspection (devall's own schema explorer parses it — tested), batches and GET; served at
`/graphql` (or `/graphql/<name>`; `config.json` sets endpoints / seed / listLength) when no stub
matched (journaled as served, not as a near miss). Stubs stay WireMock mappings with a `graphql`
request matcher (`operationName`, `operationType`, `field`, `query`, `variables`) or the WireMock
GraphQL extension's `customMatcher: gql-matcher` (normalised query equality); the
`graphql-automock` response transformer overlays the stub body's `data` (keyed by alias, lists take
the override's length, `__typename` picks the concrete type) and passes its `errors`. Admin:
`GET /__admin/devall/graphql`, `POST …/graphql/execute`. *WebSocket* — `mocks/websocket/*.json`
stubs on the HTTP port (hyper upgrade + tokio-tungstenite): request pattern on the upgrade,
subprotocol, `onConnect`, first-match `messages` rules (WireMock value matchers on the frame) with
templated / delayed / broadcast replies and close, `periodic`, `close.afterMs`, `echo`; admin
`GET /__admin/devall/websocket`, `POST …/websocket/send|close`. Differences from WireMock 4 message
stubs: no channel abstraction or HTTP-triggered message actions (use the send endpoint), stubs are
per endpoint. *gRPC* — a separate h2c listener (hyper HTTP/2; `MockConfig::grpc_port`,
`MockHandle::start_grpc/stop_grpc`, admin `POST /__admin/devall/grpc/start|stop`), `.proto` files
under `mocks/grpc` compiled with protox, messages framed by the mock itself (prost-reflect dynamic
messages) so status, headers and trailers are fully controlled, tonic-reflection v1 + v1alpha;
stubs (`*.json` under `mocks/grpc`) keyed by `package.Service/Method` with body (proto3 JSON) and
metadata matchers, `message` / `messages` + `streamIntervalMs`, status names or numbers +
`statusMessage`, `headers` / `trailers`, templates; client-streaming / bidi calls match the JSON
array of all messages and are answered after the half-close; compressed requests are refused;
unmatched calls get UNIMPLEMENTED. *MQTT* — an embedded 3.1.1 / 5 broker on rumqttc's codec (QoS
0–2, wildcards, retained, unsubscribe) with `mocks/mqtt/*.json` behaviours: `retained`,
`onSubscribe` (delivered to the subscriber), `periodic`, `rules` (topic filter + payload matcher →
templated publishes with delays); admin `GET /__admin/devall/mqtt`,
`POST …/mqtt/start|stop|publish`; no persistent sessions, wills, authentication or
MQTT-over-WebSocket. All four reload with `POST /__admin/devall/reload`; their load errors join the
mapping errors (also in `devall mock check` / `--strict`). CLI: `devall mock serve --grpc-port /
--no-grpc / --mqtt-port / --no-mqtt` (defaults 50051 / 1883 when the folders have content; busy
default ports fall back to a free one), the banner lists every endpoint and `--log` prints messages;
the Dockerfile exposes 50051 and 1883. App: Mock server → **GraphQL / WebSocket / gRPC / MQTT** tabs
(definition files edited in place with JSON-schema validation and reloaded into the running engine,
GraphQL try-a-query, WebSocket connections + push, gRPC listener controls + methods, MQTT broker
controls + clients + retained + publish, message journals) and listener chips in the server bar;
the app starts gRPC / MQTT on free ports when the project has them (`mock_grpc_start|stop`,
`mock_mqtt_start|stop`). Tests: parser, schema, validation messages, auto-mock / overrides /
introspection, matchers, project loading; end to end with real clients (reqwest, tokio-tungstenite,
devall-proto's gRPC client over reflection — unary, server streaming, client streaming, error
status, headers / trailers — and its MQTT client); vitest for the definition validators, journal
formatting and the plugins page helpers.

### 4c. Verification status (2026-10-22)

Exercised for real on this machine (Windows 11):
- Whole workspace: `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace` 910 passed / 0 failed (14 ignored = live tests that need external servers); frontend `tsc` clean, vitest 177 passed.
- Desktop app launched with `tauri dev`; all 19 sidebar sections and the newest dialogs/tabs opened over CDP with no error boundary or console errors (this found and fixed a render loop on the Tunnels page).
- Hosted relay run locally with `wrangler dev` and the real Rust CLI: `node relay/e2e/run.mjs` → 11/11 scenarios (HTTP + WebSocket tunnel, TCP tunnel, inbox buffering/ack, hosted mock publish, shares, monitors + alerts, custom domain, cloud load with verification, free-tier limits, idle heartbeat, relay restart).
- Example WASM plugin built for `wasm32-unknown-unknown`, installed with `devall plugin install`; its utility output matched Python's HMAC and its auth provider signed a real request.
- `devall run examples/jsonplaceholder` (scripts + tests), `devall mock serve` (randomized templating), `devall load run users-smoke.load.yaml` with `target: mock` (100 requests, thresholds pass).

Not exercisable here (the software isn't installed on this machine). These paths are covered by unit/integration tests with local stand-ins, plus opt-in live tests:
- Real Postgres / MySQL / SQL Server / Redis / MongoDB: set `DEVALL_TEST_PG_URL`, `DEVALL_TEST_MYSQL_URL`, `DEVALL_TEST_MSSQL_URL`, `DEVALL_TEST_REDIS_URL`, `DEVALL_TEST_MONGO_URL` and run `cargo test -p devall-db --test live`.
- Docker engine and the Docker image build (`.github/workflows/docker.yml` smoke-tests it in CI); Kubernetes clusters (`DEVALL_TEST_DOCKER=1`, `DEVALL_TEST_KUBE=1`).
- Deployed Cloudflare / Firebase / Stripe (needs the owner's accounts; see relay/README.md); TLS, Firebase ID tokens, Cloudflare for SaaS and DoH DNS verification are unit-tested with fakes.
- macOS-specific code paths (built and reviewed, not run).

## 5. Milestones

Each milestone ends with: `cargo test` green, `cargo clippy` clean, frontend typecheck + build green,
the app launches on Windows and macOS, and docs updated.

### M0 — Foundations + first vertical slice *(done)*
- [x] Workspace + crates `devall-core`, `devall-http`, `devall-utils`, `devall-cli`; app in `apps/desktop`.
- [x] Project format v1: open/create project, request tree with folder ordering, read/write/rename/move/duplicate/trash requests and folders, environments, `.gitignore` handling, atomic writes that skip unchanged content, unknown keys preserved on round trip.
- [x] Folder inheritance: headers (request overrides by name), auth (nearest non-`inherit`), folder variables.
- [x] Variable resolution: environment → folder → project layers, `{{secret.X}}` (keychain env scope → keychain all-env scope → `.env.local` → `.env`), `{{$env.X}}`, dynamic values, nested references with cycle limit, unresolved reporting, redaction of every secret value used.
- [x] HTTP engine: all methods, params, headers, JSON/text/XML/form/multipart/GraphQL/binary bodies, basic/bearer/apiKey auth, timeout, redirects, TLS verification against the OS trust store, HTTP/2, cookies, decompression, TTFB/download timing, 64 MB body cap, error classification, cancellation.
- [x] cURL import (bash, cmd `^`, PowerShell `` ` ``, `$'…'`, combined short flags, `-G`, `--json`, `-F`) and export.
- [x] Utility registry: 36 operations (encoders, JWT decode/verify/sign incl. RS/ES/EdDSA PEM keys, hashes + HMAC, UUID v4/v7, ULID, timestamps, JSON/YAML, case conversion).
- [x] CLI: `devall init`, `devall run` (filters, `--env`, `DEVALL_SECRET_<NAME>` for CI, `--fail-on-http-error`, `--json`), `devall util`, `devall import-curl`.
- [x] JSON Schemas for all project files in `schemas/`, generated from the Rust model and checked by a test.
- [x] Desktop app: wiremock-ui design system and UI kit, project picker + scratch pad + recent projects, request tree (filter, context menus, drag-and-drop reorder/move), tabs restored per project, request editor with `{{variable}}` highlighting + autocomplete, cURL paste into the URL bar, query ↔ params sync, auto-save, response viewer (pretty/raw, headers, timing, masked "request sent", inline JWT/timestamp decoding), folder settings, environments + keychain secrets with credential-leak warning, protected-environment confirmation, utilities with clipboard detection, command palette, roadmap pages for upcoming modules.
- [x] CI: clippy + tests on Windows, macOS (and Linux best-effort); frontend typecheck, tests and build.

Verified on Windows 11 by driving the real WebView2 window over the Chrome DevTools Protocol
(`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223`): open project, send, inherited
auth with missing secret, keychain round trip with masking, cURL paste → params/body → auto-save,
rename, utilities, environments, palette, protected confirm, folder settings, light/dark themes.

### M1 — HTTP client complete
OAuth2 (+PKCE), SigV4, Digest; cookie jar; proxies & client certs; history as files + diff + snapshot tests;
assertions; collection runner + `devall run --junit`; QuickJS scripts with `pm.*`; GraphQL, WebSocket, SSE;
importers (cURL ✓ in M0, Postman, Insomnia, Bruno/OpenCollection, OpenAPI, HAR, `.http`); code generation;
command palette; read-only MCP server; BYO OpenAI-compatible model + Ollama/LM Studio discovery; NL → regex/JSONPath.

### M2 — Mock server (port wiremock-ui)
`devall-mock` engine: WireMock JSON mappings, matchers (url/path/regex/query/header/cookie/body JSONPath/XPath/equalToJson),
priorities, response templating (Handlebars + Faker), scenarios, delays/faults/chaos presets, proxy & record, request journal,
near-miss report. UI: port StubList/StubEditor/StubForm/RequestsPage/NearMisses/CreateStubDialog/Scenarios.
**Request → mock → randomize** flow. Attach-to-external-WireMock mode. `devall mock serve` + Docker image.

### M3 — Traffic, local webhooks, BYO tunnels, logs
`devall-proxy` (hudsucker/rcgen MITM, local CA install helper for macOS/Windows), capture → stub/HAR;
local webhook inbox with signature verify + re-sign on replay; BYO ngrok/cloudflared/Tailscale launcher;
event store + log viewer v1 (project events, files, Docker logs).

### M4 — Group 2: hosted relay (free tier + Supporter)
`relay/`: Firebase Auth + Firestore entitlements + Cloud Function webhook capture (store-and-forward);
Cloudflare Worker + Durable Objects tunnel edge (WebSocket over 443, multiplexed streams), Firebase ID-token
verification, quotas, interstitial page for free browser traffic, Public Suffix List entry. Desktop: sign-in only
for relay features, tunnel manager, hosted inbox. Self-host guide.

### M5 — Load testing + fake data
`devall-load`: open/closed models, HDR histograms with CO correction, thresholds, channel-streamed live metrics,
run summaries in git + comparison; `devall-data`: seeded relational generator from JSON Schema/OpenAPI.

### M6 — Data & infrastructure
Postgres/MySQL/SQLite/Redis/Mongo clients (+SSH tunnels); Docker (bollard); Kubernetes (kube-rs) with
port-forward manager that writes environment `baseUrl`s; GCP emulator views.

### M7 — Local AI depth
Signed llama.cpp sidecar per platform, hardware-tiered model picker, explain-this-failure, traffic → OpenAPI,
NL → SQL/Mongo/kubectl, semantic search, MCP write tools behind confirmation.

## 6. Testing strategy
- Rust: unit tests per crate (format round-trips, ordering, variable resolution, redaction, every utility op);
  HTTP engine tests against a local `axum`/`tokio` test server; later: mock engine conformance tests derived from
  wiremock-ui's integration suite (real WireMock 3.13 / 4.0 as the oracle).
- Frontend: vitest for ported `src/lib` modules (tests come with them from wiremock-ui); typecheck in CI.
- End-to-end: dev builds expose `window.__devall` (stores); scripts drive the real app over CDP with
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223` on Windows (M1: turn the M0
  scenarios into a committed `e2e/` suite; on macOS use `tauri-driver`/WebDriver where available).
- CI matrix: `windows-latest` + `macos-latest` (+ `ubuntu-latest` best-effort).

## 7. Editor support for project files

The generated schemas let VS Code (Red Hat YAML extension) validate and autocomplete project files —
see `examples/jsonplaceholder/.vscode/settings.json`:

```json
"yaml.schemas": {
  "schemas/project.schema.json": "devall.yaml",
  "schemas/request.schema.json": "http/**/*.req.yaml",
  "schemas/folder.schema.json": "http/**/_folder.yaml",
  "schemas/environment.schema.json": "environments/*.env.yaml"
}
```

## 8. Next up (M1 order)

1. History stored per project in `.devall/history.sqlite` + "compare with previous response" (top unmet request).
2. Assertions + collection runner in core; `devall run --junit`.
3. OAuth 2.0 (auth code + PKCE with a localhost callback, client credentials, refresh) and AWS SigV4.
4. Importers: Postman v2.1, Insomnia, Bruno/OpenCollection, OpenAPI 3, HAR, `.http`.
5. GraphQL schema introspection, WebSocket and SSE clients.
6. QuickJS pre/post scripts with a `pm.*` compatibility layer.
7. File watching (`notify`) instead of refresh-on-focus; code generation; read-only MCP server; BYO-model connection.

# /dev/all — Prioritized feature map

> Mission: **give away for free what other dev tools charge for.** Money is only asked for
> things that cost us money to run (hosted servers). Everything that runs on the user's own
> machine is free, forever, with no account.

Source of truth for the evidence: [`reports/Developer toolkit competitive research.md`](../reports/Developer%20toolkit%20competitive%20research.md).
Implementation order and milestones: [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md).

## The two groups

| | **Group 1 — Free (local)** | **Group 2 — Hosted (needs our cloud)** |
|---|---|---|
| Runs on | The user's machine, the user's own cloud accounts, or the user's own AI keys | Our relay (Firebase for accounts/billing/webhook inbox, a WebSocket edge for tunnels) |
| Cost to us per user | $0 | Real, small (bandwidth, connections, storage) |
| Account required | **Never** | Only for the hosted relay (needed to manage abuse) |
| Price | Free forever, including commercial use | **Free tier** with a fair quota + a **Supporter tier** priced near cost |
| Self-hostable | n/a | **Yes** — the relay is open source; self-hosters get every Group 2 feature free |

Rules that keep the mission honest:

1. Never charge for *where data is stored*, for *history/retention on the user's disk*, for *secrets
   integrations*, for *git features*, or for *anything that runs locally*. These are exactly the
   things competitors paywall (research report, "What each category charges").
2. Group 2 pricing tracks infrastructure cost plus a little. Target: **≈ $4/month or $36/year**
   (ngrok Hobbyist is $10/month; Pinggy is $2.50/month).
3. Group 2 always has a free fallback: bring-your-own ngrok/cloudflared/Tailscale token, or self-host the relay.

## Priority legend

- **P0** — the wedge: must exist for the first public release (milestones M0–M2).
- **P1** — completes the "replace your subscriptions" story (M3–M5).
- **P2** — depth and differentiation (M6–M7).
- **P3** — later / niche.

---

## Group 1 — Free (runs locally, $0 to operate)

### Foundations (everything depends on these)

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| Project = a folder of plain YAML files, one entity per file, git-friendly ordering | P0 | Postman/Bruno git sync tiers |
| Scratch-pad default project (no folder needed), "move to project" | P0 | Postman removed its Scratch Pad in 2023 |
| Secrets as references (`{{secret.NAME}}`), values only in the OS keychain / local `.env` | P0 | Secret vaults are Enterprise at Postman/Insomnia, Ultimate at Bruno |
| Environments (variables + secret names) with protected/read-only modes | P0 | — |
| Shared Rust core used by the app **and** a `devall` CLI (CI parity) | P0 | bruno-cli, inso, newman |
| Local history store (SQLite) + event store with correlation IDs | P0 | Response history is a top unmet request (Yaak/Bruno) |
| Secret scanner before save/commit + redaction in exports and AI prompts | P1 | Enterprise add-ons elsewhere |
| Free secret-provider integrations: 1Password CLI, Bitwarden, Vault, AWS/GCP Secret Manager | P1 | Enterprise/Ultimate tiers everywhere |
| Minimal git panel (status, diff, commit) | P2 | Bruno free tier lacks push/branches |
| WASM plugin SDK | P3 | — |

### Notes and to-dos (requested 2026-10-21)

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| **Project notes**: Markdown files in `notes/` — committed and shared with the team, live preview, search, links to requests/mocks, tags | P1 | Notion/Confluence pages kept outside the repo; Postman's paid docs/collaboration |
| **Personal to-dos, Todoist style** (rebuilt 2026-10-23): user-specific, stored in the app's data folder (`todos/todos.json`, never in the repo). Inbox, a list per project, your own lists with sections and colours, labels, saved filters. **Quick add** parsed live with highlighted chips and `#`/`@`/`/` autocomplete: `Ship refunds tomorrow 5pm #Work /Backend @api p1 {fri}` — natural dates (`today`, `next fri`, `in 3 days`, `jan 15`, `15th`, `tonight`), times, **recurring** rules (`every weekday`, `every mon, wed`, `every 2 weeks`, `every month on the 1st`, `every last day`, `every first mon`, `every jan 15`, `every! 3 days` from completion, `starting`/`until`), priorities p1–p4, deadlines. Views: Today (overdue + today, "Reschedule to today"), Upcoming (week strip + 4-week agenda, drop a task on a day), lists with collapsible sections, labels, filters, search, Completed history (reopen / undo a recurring completion). Sub-tasks (drag right or `Ctrl+]` to indent), drag-and-drop between sections/lists/days, multi-select bulk actions, undo toasts, task details (Markdown description, comments, links to requests/notes/mocks/URLs, fixed time zone). Keyboard: `q` `/` `j`/`k` `e` `o` `Space` `1`–`4` `t`/`m`/`n` `Ctrl+]`/`[` `Del` `?`. **Reminders**: desktop notifications at the due time, N minutes before, and for all-day tasks at a chosen time — only while the app runs (Complete button and click-to-open on the notification). CLI: `devall todo add/list --view today\|upcoming\|inbox\|all --filter "…"/done/undo/show/edit/rm/comment/lists/labels/filters/parse`. Filter language: `today \| overdue`, `p1 & @bug`, `#Work & !@later`, `due before: next week`, `7 days`, `no date`, `search: text`, `,` for groups. Migration from v1 keeps everything (`high` → p1, `normal`/`low` → p4, `low` also gets the label `low`). | P1 | Todoist Pro ($4–5/mo for reminders, filters), separate task apps |
| **AI context**: notes and to-dos are readable by the local AI and the MCP server (with the same redaction rules), so "what's left on the payments work?" or "summarize our auth notes" work offline. To-dos carry priority, due/recurrence, deadline, labels, sections, sub-tasks and links; MCP `list_todos` takes a `filter` query and only ever exposes the open project's list; agents can also keep notes and that list current (`write_note`, `append_to_note`, `edit_note`, `add_todo`, `update_todo`, `complete_todo`, `comment_todo` — no deletes; notes with masked secrets are only edited in parts; `--read-only-notes` / the Agents tab switch turns this off). "From notes…" turns text into tasks with priority, natural-language due dates and labels | P1 | Cloud AI assistants that upload your docs |

### HTTP / API client

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| REST requests: params, headers, bodies (JSON/text/form/multipart/binary), timing, size | P0 | Postman Solo $9/mo, Team $19/user/mo |
| Auth helpers: Basic, Bearer, API key | P0 | — |
| Request tree with folders, tabs, auto-save | P0 | Tab/auto-save is a top complaint in Insomnia/Yaak/Bruno |
| Response viewer: pretty/raw/headers/timeline, inline JWT/base64/timestamp decode | P0 | — |
| Import cURL; copy as cURL / code (fetch, Python, Go, Rust…) | P0 | — |
| Imports: Postman, Insomnia, Bruno/OpenCollection, OpenAPI, HAR, `.http` | P1 | Bruno caps free OpenAPI syncs at 5/month |
| Auth: OAuth2 (auth code + PKCE with localhost callback, client creds, refresh), AWS SigV4, Digest, NTLM, mTLS | P1 | — |
| Cookie jar, proxies, client certificates, TLS options | P1 | — |
| Response history as files, diff between runs, promote to snapshot test | P1 | Snapshot tests are Pro-only in Kreya |
| Assertions + collection runner + CLI with JUnit output | P1 | Postman run limits historically; Thunder Client paywall |
| Pre/post scripts in sandboxed QuickJS with a `pm.*` compatibility layer | P1 | Yaak has no scripting at all |
| GraphQL (introspection, schema explorer), WebSocket, SSE (incl. LLM streams) | P1 | — |
| gRPC (reflection + proto files), Socket.IO, MQTT | P2 | Only Hoppscotch/ATAC have MQTT |
| OpenAPI round-trip: import, watch, export, lint | P2 | Postman/Bruno paid tiers |
| `.http` / Hurl compatibility (read/run/export) | P2 | — |

### Mock server (WireMock-compatible) — ported from `../wiremock-ui`

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| Native Rust mock engine reading WireMock JSON mappings from the project | P0 | WireMock Cloud (Free = 1k calls/mo, 3 APIs), Mockoon Cloud (Enterprise), Beeceptor |
| Stub editor (form + JSON), request journal, near-miss explanations, create-stub-from-request | P0 | wiremock-ui already has these |
| **Request → mock → randomized data** (the README's flagship flow) | P0 | Nobody does this free |
| Response templating (Handlebars + Faker helpers) | P1 | — |
| Stateful scenarios, chaos/fault & latency presets (whole API) | P1 | WireMock Cloud **Enterprise-only** |
| Proxy-and-record, OpenAPI → mocks | P1 | — |
| Attach to any external WireMock via admin API (wiremock-ui mode) | P1 | — |
| CSV/DB-backed data sources | P2 | WireMock Cloud-only |
| Docker image + CLI to run project mocks in CI | P1 | — |
| gRPC / GraphQL / WebSocket / MQTT mocks | P3 | — |

### Traffic, webhooks (local) and tunnels (bring-your-own)

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| Traffic inspector: MITM proxy with local CA, per-app capture, breakpoints, Map Local/Remote | P1 | Proxyman $89, HTTP Toolkit Pro, Charles $50, Fiddler $7–37/mo |
| Capture → stub / HAR / OpenAPI / load scenario | P1 | HTTP Toolkit Pro (rules from traffic), Postman Insights (Enterprise) |
| Local webhook inbox (LAN/localhost) with unlimited local history | P1 | webhook.site (100 req/URL free), Beeceptor (50/day) |
| Webhook signature verify + **re-sign on replay** (Stripe, GitHub, Shopify, Slack, Twilio, Standard Webhooks) | P1 | No surveyed tool does re-signing |
| Bring-your-own tunnel: start ngrok / cloudflared / Tailscale Funnel with the user's token | P1 | — |
| Spec ↔ mock ↔ traffic drift checks (validation proxy) | P2 | Optic archived; Postman Insights Enterprise |

### Load testing

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| Load test any saved request/flow; open-model (arrival-rate) by default | P1 | k6 Cloud $0.15/VUh, Gatling €89–356/mo, Artillery $199+/mo |
| HDR histograms with coordinated-omission correction; thresholds → exit code | P1 | — |
| Run summaries committed to git + run comparison / baselines | P1 | Paid SaaS everywhere |
| Baseline against the local mock | P1 | — |
| Distributed agents via Docker/K8s on the user's infra | P3 | Gatling Enterprise, k6 operator |

### Utilities (inline everywhere + standalone)

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| Encoders: Base64/Base64URL, URL, HTML entities, hex | P0 | DevUtils $29–39 |
| JWT decode/verify/sign (HS/RS/ES, JWKS fetch, expiry warnings) — never leaves the machine | P0 | jwt.io (token leakage risk) |
| Hashes/HMAC, UUID v4/v7, ULID, timestamps, JSON/YAML format & convert | P0 | — |
| Clipboard auto-detect + command palette + inline decode in every view | P1 | DevToys Smart Detection |
| Regex sandbox (Rust + PCRE/JS flavors), explain, test cases | P1 | regex101 Pro |
| Text & semantic JSON diff | P1 | Diffchecker $15/mo, Kaleidoscope $8/mo |
| jq / JSONPath playground over live responses | P1 | — |
| cURL ↔ code, JSON → types (quicktype-style) | P1 | — |
| CyberChef-style recipes (chained transforms), cron parser, X.509/TLS chain, DNS, CIDR | P2 | — |

### Fake data and logs

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| Seeded, relational fake-data generator from JSON Schema/OpenAPI/DB schema | P1 | Mockaroo $60–7,500/yr |
| Log viewer: project event stream, files (GB-scale), Docker logs; JSON-aware filtering | P1 | — |
| SQL over logs/responses/CSV (embedded DuckDB) | P2 | lnav/Dozzle have it |

### Data & infrastructure clients

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| Postgres, MySQL, SQLite clients; saved queries as files; read-only by default | P1 | TablePlus $99, Beekeeper $9–18/mo, DataGrip |
| Redis browser (all types, streams, pub/sub) and MongoDB (find/aggregate) | P1 | Studio 3T $499/yr |
| SSH tunnels for DB connections | P1 | — |
| Docker viewer (containers, logs, exec) on any socket (Docker/Podman/OrbStack) | P1 | Docker Desktop Pro $9/mo (we don't replace the runtime) |
| Kubernetes viewer + **port-forward manager that fills HTTP environments** | P1 | Lens $25–60/seat/mo (orgs > $10M) |
| GCP emulator-first views: Pub/Sub UI, Cloud Storage, Logging tail, Secret Manager import | P2 | Pub/Sub emulator has no UI at all |
| SQL Server driver | P3 | — |

### AI (bring your own model) and agents

| Feature | Priority | Replaces / what others charge |
|---|---|---|
| Connect any OpenAI-compatible endpoint; auto-discover Ollama (≥0.40) and LM Studio on macOS/Windows | P1 | Postman AI credits ($0.035–0.05 overage) |
| NL → regex / JSONPath / jq checked against samples; schema-constrained mock data | P1 | — |
| MCP server (agents can run saved requests, read mocks, write notes and to-dos); HTTP writes need in-app confirm | P1 | WireMock MCP = Enterprise, Fiddler MCP = Pro |
| Managed signed llama.cpp sidecar (Metal on macOS; CPU/Vulkan on Windows; CUDA on demand) + hardware-tiered model picker | P2 | — |
| Explain-this-failure (request + mock + logs + pod events, cited) | P2 | — |
| Traffic → OpenAPI (deterministic clustering + LLM naming, validated) | P2 | Postman Insights (Enterprise) |
| NL → SQL / Mongo / kubectl checked with EXPLAIN / dry-run | P2 | Cloud-only AI in DB GUIs |
| Local semantic search across the project (EmbeddingGemma-2) | P3 | — |

Default models (Apache-2.0 only for one-click download; see research report "hardware-tiered lineup"):
Qwen3.5 / Gemma 4 on 8–16 GB, Qwen3.8-27B on 24 GB+, Qwen3-Coder-30B-A3B for autocomplete on 24 GB+,
Mellum-4b as the small-machine autocomplete fallback, EmbeddingGemma-2 for search. **No Qwen2.5 models.**

---

## Group 2 — Hosted (needs our cloud; free tier + Supporter tier)

| Feature | Priority | Free tier (fair quota) | Supporter tier (~$4/mo) | Free fallback |
|---|---|---|---|---|
| **Hosted webhook inbox**: public URL per project, events buffered while the laptop is off/asleep, forwarded + verified locally | P1 | 1 URL, 24 h buffer, ~500 events/day | Stable per-project URLs, 30-day buffer, higher limits | Local inbox + BYO tunnel; self-host relay |
| **Reverse tunnel** (HTTP / WebSocket / SSE) to localhost — the ngrok replacement | P1 | Random subdomain, session-limited, ~1 GB/mo, interstitial warning page for browsers | Reserved subdomains, no interstitial, more bandwidth, multiple tunnels | BYO ngrok/cloudflared/Tailscale; self-host relay |
| Accounts + licensing (Firebase Auth), Supporter billing (Stripe via Firebase extension) | P1 | — | — | n/a |
| Custom domains for tunnels (CNAME, managed certificates via Cloudflare for SaaS) | P2 | — | ✓ (2 domains) | self-host (`CUSTOM_HOSTS`) |
| Raw TCP tunnels (databases, SSH) — TCP over WebSocket, owner account or access secret | P3 | connect to others' tunnels | ✓ publish (20 connections/tunnel) | self-host |
| Publicly hosted mocks (mock stays up when laptop is off; documented WireMock subset, journal) | P3 | 1 small mock (50 stubs, 1 MB, 5 000 req/day) | 10 mocks, 1 000 stubs, 20 MB, 100 000 req/day | Docker image on your own host; self-host |
| Shareable request/response snapshots (expiring, redacted, revocable links) | P3 | ✓ (20 links, 256 KB, ≤ 7 days) | 500 links, 5 MB, ≤ 90 days | export file; self-host |
| Scheduled monitors (saved requests + assertions from the edge, webhook/Slack alerts) | P3 | — | 20 monitors every 5 min (20 requests) | `devall run` in your own CI; self-host |
| Cloud load-generator regions (open-model bursts from the edge, verified targets only) | P3 | — | ≤ 3 regions, ≤ 60 s, ≤ 100 req/s, 20 runs/day | `devall load` / distributed agents on your own machines; self-host (every region, private targets) |

**How cloud load regions stay safe:** a "load burst from the edge" must not become a free
stress-testing (DDoS) service aimed at someone else's URL from Cloudflare's shared IPs, so — like
loader.io and k6 cloud — **no load is sent until the account proves it controls the target**: a
token served at `<origin>/.well-known/devall-load-verification.txt` or a DNS TXT record
`_devall-load.<host>` (re-verified every 30 days). Runs are short and bounded (duration, total
rate, regions, runs per day, one at a time per account), the hosted relay never targets private,
loopback or link-local addresses, every request carries `x-devall-load: <run id>`, and the
operator has a kill switch. Placement is honest about its limits: Durable Object `locationHint`
only suggests a coarse region when an object is created; each result lists the data center a
region actually ran in when the relay can detect it. Local load testing stays free and unlimited
(`devall load`, distributed agents); see `relay/README.md` → Abuse policy.

**Infrastructure decision (from the research):** Firebase handles Auth, entitlements and the
store-and-forward webhook inbox (Cloud Functions + Firestore fit the free quotas). Firebase **cannot**
host a usable reverse tunnel (Hosting cuts requests at 60 s; no WebSocket support in Functions), so the
tunnel edge runs on Cloudflare Workers + Durable Objects (~$5/month flat, hibernating idle sockets, no
bandwidth charges) behind a `RelayProvider` interface, verifying Firebase ID tokens. Cloud Run is the
"stay inside Google" alternative but bills every connected socket as an active instance.

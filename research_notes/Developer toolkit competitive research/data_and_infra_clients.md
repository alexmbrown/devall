# Data & Infrastructure Clients (SQL, Redis, MongoDB, Kubernetes, Docker, GCP) and Rust Building Blocks — state as of Oct 2026

_Research date: 2026-10-21. Primary sources (vendor pricing pages, GitHub READMEs, crates.io API, docs.rs, official docs) were preferred and fetched directly where possible. Items explicitly dated before 2025 are flagged "(pre-2025, possibly outdated)". The shared web-search budget ran out partway through, so some Reddit/HN sentiment and a few product details could not be gathered; those are listed under each section's Gaps. GitHub star counts are as displayed on repo pages fetched in Oct 2026._

---

## 1. SQL clients: features, pricing, open-source status, tech stack

### Takeaway
The market splits into native paid apps (TablePlus, Sequel Ace, HeidiSQL), heavy Java/JetBrains IDEs (DBeaver, DataGrip), and Electron open-core apps (Beekeeper Studio, DbGate, Antares, pgAdmin's desktop runtime). Nearly all now ship AI/NL-to-SQL, and in 2026 several also ship MCP servers. AI is usually a paid tier and calls a cloud LLM. Azure Data Studio was retired in Feb 2026 and Arctype is gone, leaving room for a fast, lightweight, git-friendly client.

### Cited Findings

**TablePlus (native; proprietary; perpetual license)**
- Pricing (official page, fetched Oct 2026): Basic $99/license (1 device), Standard $129 (2 devices, discounted from $198), Team $79/seat with a 3-seat minimum. Existing customers pay $79 per extra device or $59/device to renew. — [TablePlus pricing](https://tableplus.com/pricing)
- License: perpetual, no subscription. It includes 1 year of updates and support, and the app keeps working after that. 7-day money-back guarantee. iOS app is free for PC-license owners (Basic includes 2 iOS devices, Standard 4). — [TablePlus pricing](https://tableplus.com/pricing)
- Free trial limits: 2 open tabs, 2 windows, 2 advanced filters. Filters are not available in the free Windows version. — [TablePlus pricing](https://tableplus.com/pricing)
- Engines: MySQL, PostgreSQL, SQLite, SQL Server, MariaDB, Redshift, CockroachDB, Oracle, Vertica, BigQuery, ClickHouse, TursoDB, plus the non-relational **Redis, MongoDB, Cassandra**. Native apps for macOS (12+), Windows, Linux, iOS. Features include "code review" of pending changes before commit, Safe mode for production, SQL-dump import/export, beta JavaScript plugins, inline editing, advanced filters, autocomplete and formatting, and a metrics board. — [tableplus.com](https://tableplus.com/)
- Third-party reviews praise its speed and keyboard-driven workflow, and criticize the restrictive free tier compared with Beekeeper Community. — [devtoollab](https://devtoollab.com/blog/best-sql-clients), [Capterra reviews](https://www.capterra.com/p/170642/TablePlus/reviews/?page=2) (secondary)

**DBeaver (Java/Eclipse RCP; Community is Apache-licensed, PRO is commercial)**
- Community latest is 26.2.3 (Oct 18, 2026), "free and open source" under ASL. Community includes AI Chat and the `@ai` command. PRO-only: NoSQL (MongoDB, Cassandra, Redis, CouchDB), cloud DBs (Redshift, BigQuery…), native AWS/GCP/Azure support, Visual Query Builder, Task Scheduler, advanced AI, ODBC. — [dbeaver.io](https://dbeaver.io/)
- PRO pricing per DBeaver's comparison page (via search summary, Oct 2026): Lite $113/yr or $12/mo, Enterprise $255/yr or $26/mo, Ultimate $510/yr (adds Cloud Explorer for AWS/GCP/Azure and cloud storage), Team Edition $1,630/yr. An older snapshot of the same page shows $110/$250/$500. — [DBeaver editions](https://dbeaver.com/edition/), [PricingSaaS](https://pricingsaas.com/companies/dbeaver)
- 26.1 added external **MCP server** support in AI Chat, with tools that run after user confirmation, and context-aware AI SQL suggestions. 26.2 (Sept 2026) added per-conversation AI engine profiles and an option to open AI-generated scripts in a new console. These notes are from the commercial editions. — [DBeaver Lite 26.2](https://dbeaver.com/dbeaver-lite-26-2/), [DBeaver Lite 26.1](https://dbeaver.com/dbeaver-lite-26-1/)
- Community AI is "basic AI assistant for SQL generation" with OpenAI and GitHub Copilot engines. Chat, smart object description and error-fixing are commercial. — [dbeaver.io](https://dbeaver.io/) (via search summary)

**Beekeeper Studio (Electron + Vue; GPLv3 Community plus commercial source-available features)**
- Official pricing (fetched Oct 2026), per user: Community free; Indie $9/mo billed yearly or $18 monthly (max 2 users, 1 device); Professional $14 or $28; Business $18 or $35; Enterprise "starts at $4,999/month". 14-day trial, 30-day refund. 12 months of payment grants perpetual use of versions released in that period. — [Beekeeper pricing](https://www.beekeeperstudio.io/pricing)
- Community is free for individuals, students, non-profits and businesses with <10 employees and/or <$1M revenue. Community has the SQL editor, table editing, schema tools, query and connection folders, unlimited connections and tabs, and SSH tunnels. Indie adds JSON sidebar, import/export, backup/restore, and editing of query results. Professional adds **"AI Shell"** and cloud workspaces. Business adds Kerberos, IAM, Entra ID and SSO auth. — [Beekeeper pricing](https://www.beekeeperstudio.io/pricing)
- Stack: "Electron app with Vue.js" monorepo (Node 22, Yarn). About 23.8k stars and 1.1k open issues. Engines: PostgreSQL, MySQL, SQLite, SQL Server, Redshift, CockroachDB, MariaDB, TiDB, BigQuery, **Redis**, Oracle, Cassandra, ScyllaDB, Firebird, LibSQL, ClickHouse, DuckDB, SQL Anywhere, **MongoDB**, Trino, SurrealDB, Snowflake, DynamoDB (beta). Features include saved queries and query run-history. — [GitHub beekeeper-studio](https://github.com/beekeeper-studio/beekeeper-studio)
- Criticism: Electron memory use is higher than native TablePlus, and Community lacks AI and cloud workspaces. — [devtoollab](https://devtoollab.com/blog/best-sql-clients) (secondary)

**DataGrip (JetBrains; JVM)**
- Free for non-commercial use (learning, hobby, OSS, content creation) with the full feature set. The license is one year and auto-renews. Commercial work needs a paid license. Sources conflict on the start date: blog URLs dated both 2024/10/01 and 2025/10/01, with 2025 more likely. — [JetBrains DataGrip blog](https://blog.jetbrains.com/datagrip/2025/10/01/datagrip-is-now-free-for-non-commercial-use/), [DataGrip 2025.3 help](https://www.jetbrains.com/help/datagrip/2025.3/register.html)

**DbGate (Svelte + Node/Express + Electron; GPL-3.0 Community, Premium commercial)**
- Official pricing: Community free/OSS. Premium $12/mo or $120/yr per user. Team Premium $15/user/mo or $150/user/yr (2+ users), with a 30-day trial. Premium adds query designer, charts, backup/restore and "AI-powered DB chat". Community is limited to 5 connections and 5 SQL files in DbGate Cloud. — [DbGate pricing](https://www.dbgate.io/pricing/)
- Stack and features: Svelte frontend, Node.js/Express backend, Electron desktop, **web/Docker deployment mode**, plugin system. Supports MySQL, PostgreSQL, SQL Server, Oracle, **MongoDB, Redis**, SQLite, CockroachDB, MariaDB, ClickHouse, Cassandra, DuckDB, Firebird (Redshift, CosmosDB, libSQL/Turso and Firestore are premium). Features: ER diagram, schema compare, import/export (CSV, Excel, JSON, NDJSON, XML, DBF), NDJSON "archive" folders, charts, map view. About 7.3k stars. — [GitHub dbgate](https://github.com/dbgate/dbgate)

**Antares SQL (Electron + Vue; MIT)**
- Supports MySQL/MariaDB, PostgreSQL, SQLite, Firebird, DuckDB and SQL Server. README still says "in development state". 2.7k stars. — [GitHub antares-sql](https://github.com/antares-sql/antares)
- The latest release a third party could find was v0.7.35 (June 2025), with the last push in Oct 2025. 2026 activity is unconfirmed. — [dev.co](https://dev.co/databases/open-source/antares) (secondary)

**Sequel Ace (native macOS, Objective-C; MIT)**
- MySQL 5.7+ and MariaDB 10.0+ only. Requires macOS 13.5+. Ships a **built-in MCP server for AI agents** that is off by default, localhost-only and read-only unless changed. 7.6k stars. — [GitHub Sequel-Ace](https://github.com/Sequel-Ace/Sequel-Ace)

**HeidiSQL (Delphi on Windows, Lazarus/FPC on Linux and macOS; free/OSS)**
- The Windows build uses Delphi 12.3, and Lazarus 4.4 compiles the Linux and macOS releases. Latest shown is 12.21 ("03 Aug", year not displayed). 12.19 added Linux arm64. Supports MariaDB, MySQL, MS SQL, PostgreSQL, SQLite, Interbase, Firebird, ProxySQL, Redshift. — [heidisql.com](https://www.heidisql.com/)
- The first native Linux version shipped in May 2025. A macOS arm64 preview and a v13 Windows preview exist. — [Wikipedia](https://en.wikipedia.org/wiki/HeidiSQL), [Lazarus forum](https://forum.lazarus.freepascal.org/index.php/topic,71037.0.html)

**Azure Data Studio — retired**
- Retired **Feb 28, 2026**. Microsoft points users to VS Code with the MSSQL extension, or SSMS 21. Installed copies still run but get no patches or support. — [Microsoft product page](https://azure.microsoft.com/products/data-studio), [Directions on Microsoft](https://www.directionsonmicrosoft.com/reports/azure-data-studio-is-retiring-what-you-need-to-do-and-when/)
- Analysts say no migration option has the same feature set. Some functions, such as SQL Agent jobs, need Windows-only SSMS. — [DevClass, Feb 2025](https://www.devclass.com/databases/2025/02/11/microsoft-drops-azure-data-studio-in-favour-of-visual-studio-code-extension-despite-missing-features/1629157)

**pgAdmin 4 (Python Flask + React web app, Electron desktop runtime)**
- Python/Flask server side, React client, and an Electron runtime in `runtime/` for desktop. 3.9k stars. — [GitHub pgadmin4](https://github.com/pgadmin-org/pgadmin4)

**Outerbase / Drizzle Studio / Arctype**
- Cloudflare acquired Outerbase (announced April 7, 2025). Its tech goes into D1, Durable Objects and the Agents SDK. OSS repos continue and can be self-hosted, but the hosted Outerbase cloud was scheduled to shut down Oct 15 (2025). — [ITPro](https://itpro.com/software/development/cloudflare-outerbase-acquisition-development), [IT Brief](https://itbrief.co.uk/story/cloudflare-boosts-ai-tools-with-outerbase-acquisition)
- Drizzle Studio is launched with `npx drizzle-kit studio`, served at local.drizzle.studio, and reads the drizzle config/schema. It browses and edits data for PostgreSQL, MySQL and SQLite. A self-hosted "Drizzle Gateway" serves the same UI. The ORM is OSS but Studio has commercial plans. — [Drizzle docs](https://orm.drizzle.team/drizzle-studio/overview), [pkgpulse 2026](https://www.pkgpulse.com/guides/drizzle-studio-vs-prisma-studio-vs-dbgate-2026)
- arctype.com now returns 301 to clickhouse.com (checked Oct 2026). — [arctype.com redirect](https://arctype.com/)

**SQLTools (VS Code extension; MIT)**
- Maintained by Matheus Teixeira and George James Software, who are "looking for people to maintain the official drivers". Official drivers: CockroachDB, MariaDB, SQL Server, MySQL, TiDB, PostgreSQL, SQLite. About 25 community drivers exist, including DuckDB and ClickHouse, and some "Pro" drivers are paid. 1.8k stars and 320 open issues. — [GitHub vscode-sqltools](https://github.com/mtxr/vscode-sqltools)

### Inferences
- **AI is now table stakes but cloud-bound and usually paywalled**: DBeaver Community has basic AI but chat is paid, Beekeeper's AI Shell is Professional+, and DbGate's AI chat is Premium. A free, local-LLM NL→SQL feature that never sends schema off-machine would stand out.
- **MCP is the new integration surface**: DBeaver 26.1 consumes external MCP servers and Sequel Ace exposes one. /dev/all could expose its connections, read-only by default, to its own local LLM and to external agents through MCP.
- **Lightweight + multi-engine + affordable is still underserved**: TablePlus is fast but paid per device. Beekeeper and DbGate are Electron. DBeaver is heavy. HeidiSQL and Sequel Ace cover limited platforms or engines. A Tauri/Rust client has a credible "native-feeling, small" angle.
- **Storage formats to emulate**: DbGate's NDJSON "archive" folders and Beekeeper's saved-query folders are close to /dev/all's "plain files in git" model. How each tool persists connections and queries on disk was not checked (see Gaps).

### Gaps
- DataGrip commercial pricing could not be verified (the JetBrains buy page did not render).
- Whether DBeaver Community has ER diagrams and SSH tunnels was not confirmed on the 2026 page. Historically it has both (pre-2025 knowledge, unverified here).
- Storage formats for connections and saved queries (file-based vs. internal DB, git-friendliness) were not researched for any client.
- Antares' 2026 maintenance status is unconfirmed.
- HeidiSQL 12.21's year is not shown on the site (likely 2026, unconfirmed).

---

## 2. Redis GUI clients

### Takeaway
RedisInsight (official, free, SSPL, Electron) leads on diagnostics and AI (Redis Copilot). Another Redis Desktop Manager (MIT, Electron, 34.8k stars) is the popular lightweight browser and is still updated in 2026. RESP.app now redirects to RedisInsight, and Medis is effectively stalled. Multi-engine clients (TablePlus, Beekeeper, DbGate) add basic Redis support, but DBeaver puts Redis in PRO.

### Cited Findings
- **RedisInsight**: license **SSPL**, "Built with … Electron, Monaco Editor and NodeJS", 8.9k stars. Features: key browser with JSON, Hex and ASCII views; CRUD for all core types plus JSON, vector sets and arrays; real-time profiler; SlowLog analysis; Pub/Sub; bulk delete by filter; Workbench CLI with autocomplete and visualizations; vector search index management; visualization plugins. — [GitHub RedisInsight](https://github.com/redis/RedisInsight)
- RedisInsight "database analysis" covers memory, data-type distribution and security. **Redis Copilot** has a general chatbot plus a "data chatbot" that builds queries from natural language. It also supports Redis Data Integration pipelines. — [Redis Insight docs](https://redis.io/docs/latest/develop/tools/insight)
- RedisInsight 3.0.0 (Nov 2025) brought a refreshed UI, and 3.0.2 (Jan 2026) fixed CPU metrics. This comes from a third-party changelog summary and is unverified. — [Redis Insight docs](https://redis.io/docs/latest/develop/tools/insight) (search summary)
- **Another Redis Desktop Manager (ARDM)**: MIT, Electron (Element UI), 34.8k stars, 2.7k forks. Features: Cluster (2020) and Sentinel (2021), SSH tunnel and SSH+Cluster, streams with groups and consumers, subscribe and Monitor views, memory analysis (including folder-level), slow log, SSL/TLS, read-only mode, import/export, custom viewers, "won't crash when loading massive keys". Its feature log shows TimeSeries, Vector and ARRAY support on 2026-08-15 and connection groups on 2026-07-29. — [GitHub ARDM](https://github.com/qishibo/AnotherRedisDesktopManager)
- Comparison articles call ARDM the lighter data-browsing tool and RedisInsight the better choice for profiling and diagnostics. — [DbGate blog, Aug 2025](https://www.dbgate.io/news/2025-08-11-free-redis-clients/), [Dragonfly guide](https://www.dragonflydb.io/guides/top-open-source-redis-desktop-managers) (vendor blogs)
- **Medis**: MIT, Electron, React and Redux, built on ioredis and ssh2. 11.7k stars. Uses SCAN so it can handle millions of keys. Has SSH tunnels and JSON/MessagePack viewers. Its roadmap items (Windows/Linux builds, cluster management, Lua editor) were never delivered. The original Mac App Store version was paid, and a native "Medis 2" rewrite is free on the App Store. — [GitHub medis](https://github.com/luin/medis)
- **RESP.app** (formerly Redis Desktop Manager): resp.app returns 301 to Redis's RedisInsight page (checked Oct 2026). — [resp.app redirect](https://resp.app/)
- **TablePlus** lists Redis among supported engines. — [tableplus.com](https://tableplus.com/) **DbGate** and **Beekeeper** also list Redis. — [GitHub dbgate](https://github.com/dbgate/dbgate), [GitHub beekeeper-studio](https://github.com/beekeeper-studio/beekeeper-studio) **DBeaver** puts Redis in PRO only. — [dbeaver.io](https://dbeaver.io/)
- Rust drivers: `fred`'s description is "An async client for Redis and Valkey". — [crates.io fred](https://crates.io/api/v1/crates/fred)

### Inferences
- The "must-have" Redis feature set for a lightweight viewer is: SCAN-based key tree with pattern grouping; type-aware editors for string, hash, list, set, zset, stream, JSON and (now) vector sets; TTL editing; a CLI/workbench; pub/sub subscribe/publish; MONITOR; SLOWLOG; MEMORY USAGE/INFO-based analysis; and cluster, sentinel, TLS and SSH support. ARDM shows this is achievable as a free tool. The differentiator would be integration (log viewer for MONITOR/pub-sub, mock-data seeding) rather than breadth.
- RedisInsight's SSPL license rules out embedding or forking its code into a commercial product. Clean-room is required.
- Supporting Valkey explicitly, which `fred` already advertises, is a cheap signal of neutrality after the Redis licensing churn.

### Gaps
- No direct Reddit or HN sentiment was collected (search budget exhausted).
- RedisInsight's latest version and date and any 2026 licensing changes are unverified.
- Whether TablePlus's Redis support covers streams, pub/sub and cluster was not verified.

---

## 3. MongoDB GUI clients

### Takeaway
MongoDB Compass (free, SSPL, Electron) is the default and has solid aggregation, schema and explain tooling plus NL query generation, which is cloud AI via Azure OpenAI. Studio 3T is the premium IDE ($499–$699/user/yr). Robo 3T is dead. NoSQLBooster sells perpetual licenses. Local-LLM NL→Mongo query generation that keeps schema on the machine is a clear gap.

### Cited Findings
- **Compass** is a free GUI. Features: schema analysis by sampling (ranges, frequent values, missing or inconsistent types); query bar; indexes; real-time server metrics; visual **Explain Plan**; **aggregation pipeline builder** with per-stage output previews; import/export; **natural-language query generation**. — [MongoDB Compass product page](https://www.mongodb.com/products/tools/compass)
- The aggregation builder offers 200+ operators and exports pipelines to multiple programming languages. Aggregation explain plans arrived in Compass 1.32.0 (2022, pre-2025). — [MongoDB Compass features](https://mongodb.com/products/tools/compass/features), [Compass release notes](https://docs.mongodb.com/compass/beta/release-notes)
- NL querying generates filters **and aggregation pipelines**. It uses **Azure OpenAI**, and prompts plus schema details are sent to Microsoft and OpenAI (not stored or used for training). It is labeled experimental and may be inaccurate. A separate "Intelligent Assistant" for Q&A, debugging and performance advice is in public preview. — [Compass NL query docs](https://www.mongodb.com/docs/compass/query-with-natural-language/), [Compass AI assistant docs](https://mongodb.com/docs/compass/query-with-natural-language/compass-ai-assistant)
- Compass stack: Electron, React, TypeScript (hadron-ipc, compass-components), plus a `compass-web` browser build. License **SSPL**. 1.5k stars. — [GitHub mongodb-js/compass](https://github.com/mongodb-js/compass)
- **Studio 3T** (official buy page): Desktop IDE Professional **$499/user/yr**, Ultimate **$699/user/yr** (adds security/auth, bi-directional migration, full SQL, team features), billed yearly with no auto-renew. "3TL Bridge" CDC to Kafka and Pub/Sub costs from $1,000/pipeline. Community Edition is for non-commercial use (students, hobbyists, small orgs). The self-hosted Build Track AI assistant uses Amazon Bedrock, OpenAI or Gemini, "no on-premises model option". — [Studio 3T buy](https://studio3t.com/buy/)
- **Robo 3T** is discontinued, with 1.4.4 as the last release. It could not work with the Node-based `mongosh`, so Studio 3T released "Studio 3T Free" instead (pre-2025, possibly outdated). Today's Studio 3T page lists a non-commercial "Community Edition" instead. — [Robomongo blog](https://blog.robomongo.org/studio3t-free/), [Studio 3T buy](https://studio3t.com/buy/)
- **NoSQLBooster**: after the 30-day trial it becomes a limited free edition usable for personal or commercial work. Paid Personal and Commercial licenses differ in Kerberos/LDAP auth, CLI tasks, scheduling and code conversion. Third parties list perpetual pricing of $99–$2,500 (unverified). — [NoSQLBooster editions](https://nosqlbooster.com/compareEditions), [AlternativeTo](https://alternativeto.net/software/mongobooster/about)
- Multi-engine support: Beekeeper, DbGate and TablePlus list MongoDB. DBeaver puts MongoDB in PRO. — [GitHub beekeeper-studio](https://github.com/beekeeper-studio/beekeeper-studio), [GitHub dbgate](https://github.com/dbgate/dbgate), [tableplus.com](https://tableplus.com/), [dbeaver.io](https://dbeaver.io/)

### Inferences
- A minimal competitive Mongo view needs: a collection browser with JSON/tree/table views; a filter/sort/projection query bar; an aggregation editor with stage previews (`$limit` per stage); explain output; index list; schema sampling (`$sample` plus type histogram); and import/export of JSON/NDJSON/CSV. All are doable on the official Rust `mongodb` driver (Section 7).
- Compass's AI sending schema to Azure OpenAI, and Studio 3T having no on-prem model, are privacy objections a local-LLM approach answers directly.
- SSPL on Compass means no code reuse. The `mongosh` shell is a separate Node project, and an embedded JS shell is probably out of scope for a Rust app (see Gaps).

### Gaps
- Current `mongosh` status, and whether a Rust app could embed or emulate it, was not researched.
- Studio 3T Community Edition's feature list was not shown on the buy page.
- No 2026 Reddit/HN sentiment collected for Mongo GUIs.

---

## 4. Kubernetes GUIs: Lens, Freelens, Headlamp, k9s, Aptakube, others; licensing controversies

### Takeaway
Lens (Mirantis, now marketed at lenshq.io) is proprietary: free only for orgs under $10M revenue, $25–$60/seat/mo otherwise. The community moved to **Freelens** (MIT, Electron). **Headlamp** (kubernetes-sigs, Apache-2.0) is the official successor to Kubernetes Dashboard, which was archived on Jan 21, 2026. **k9s** remains the TUI favourite. **Aptakube** and **kftray** prove that Tauri/Rust Kubernetes desktop apps are viable. Monokle and Octant are dead.

### Cited Findings

**Lens Desktop (Mirantis)**
- k8slens.dev/pricing now redirects 301 to **lenshq.io/pricing**. Plans: **Personal free** (students, hobbyists, small orgs). **Plus $25/mo** billed annually ($300) or $30 monthly (individual). **Pro $25/seat/mo** ($300/yr) or $30. **Enterprise $50/seat/mo** ($600/yr) or $60. Orgs with **>$10M annual revenue or funding** must pay, with a 30-day evaluation. — [Lens pricing](https://lenshq.io/pricing)
- Personal includes multi-cluster, resource views, real-time logs, Smart Terminal, Helm charts and port forwarding. Plus adds "Ask AI", GitOps (Flux/Argo CD), EKS/AKS/GKE integration and Security Center (CVEs). Pro adds a **built-in MCP Server** and team features. Enterprise adds air-gapped mode, SSO/SCIM, etc. — [Lens pricing](https://lenshq.io/pricing)
- Subscriptions are per user across machines, with a 14-day trial. Lens can run standalone with cloud services disabled. — [Lens FAQ](https://docs.k8slens.dev/faq/subscription-and-licensing/)
- Controversy: Mirantis acquired Lens in 2020. With Lens 6 it introduced an EULA requiring **sign-in and payment for orgs >$10M revenue**. It removed features (logs, terminal) from OSS OpenLens, so users had to patch the code. — [The New Stack](https://thenewstack.io/the-open-source-ethos-why-lens-made-a-mistake/) (pre-2025, possibly outdated)
- Lens later stopped publishing the source OpenLens depended on. The OpenLens repo now says "Lens closed its source code… do not expect any more updates". — [DevGenius: move to Freelens](https://blog.devgenius.io/i-recommended-openlens-now-its-time-to-move-to-freelens-ae1a86773f80)

**Freelens (OpenLens fork; MIT; Electron)**
- MIT ("Freelens Authors 2024–2026, OpenLens Authors 2022"), 5.7k stars, 164 open issues. **Bundles kubectl**, and the Flatpak also bundles `helm` plus host wrappers for `aws`, `doctl`, **`gke-gcloud-auth-plugin`** and `kubelogin`. Requires K8s 1.22+, and falls back to bundled kubectl for older clusters. Extensions ported from OpenLens. — [GitHub freelens](https://github.com/freelensapp/freelens)
- Releases: v1.9.0 (May 5, 2026: pod resize, log wrap); v1.10.0 (Jun 19, 2026: metrics time-range selector, custom kubectl mirror for air-gap, Electron 41/Node 24); v1.10.2 (Jun 28: signed Windows binaries to avoid AV false positives); **v1.10.3 (Jul 7, 2026)**, bundling kubectl v1.36.2 and Helm v4.2.2 on Electron 41.10. — [Freelens releases](https://github.com/freelensapp/freelens/releases)

**Headlamp (kubernetes-sigs; Apache-2.0)**
- **Kubernetes Dashboard archived Jan 21, 2026** ("no longer maintained due to lack of active maintainers"). Its README recommends Headlamp, now under SIG UI. — [GitHub kubernetes/dashboard](https://github.com/kubernetes/dashboard)
- Kubernetes blog (Jun 1, 2026): Headlamp adds multi-cluster support, an optional app-centric "Projects" view, relationship maps, a plugin catalog (Flux GitOps, **AI Assistant**), in-cluster and **desktop app** modes (desktop uses an existing kubeconfig and deploys nothing to the cluster), and RBAC-governed editing. Dashboard was single-cluster only. — [Kubernetes blog](https://kubernetes.io/blog/2026/06/01/dashboard-to-headlamp/)
- Repo has `backend/` (Go) and `frontend/` (React) directories. Features include multi-cluster, logs, exec and a resource editor. 7.5k stars. — [GitHub headlamp](https://github.com/kubernetes-sigs/headlamp)
- **v0.46.0 (Oct 15, 2026)**: service-backed **port-forwards auto-reconnect after pod restart**, plugin credential isolation, and desktop builds can set "an explicit list of approved commands". **v0.45.0 (Aug 20, 2026)**: desktop startup cut memory by about 60 MiB RSS. **v0.44.0 (Jul 29, 2026)**: configurable node-shell image. — [Headlamp releases](https://github.com/kubernetes-sigs/headlamp/releases)
- Headlamp is offered as a VMware vSphere Kubernetes Service add-on (Aug 2026). — [VMware blog](https://blogs.vmware.com/cloud-foundation/2026/08/12/headlamp-as-a-vmware-vsphere-kubernetes-service-add-on/)

**k9s (TUI; Go; Apache-2.0)**
- 34.9k stars. Features: logs (current and previous, wrap, timestamps), shell/attach, **port-forward (Shift-F)**, XRay and Pulses views, plugins bound to hotkeys, HTTP benchmarks against port-forwards (Hey), Popeye sanitizer, skins, `--readonly`. — [GitHub k9s](https://github.com/derailed/k9s)
- v0.51.0 is a 2026 maintenance release with custom-resource jump support. — [newreleases k9s v0.51.0](https://newreleases.io/project/github/derailed/k9s/release/v0.51.0)

**Aptakube (Tauri; proprietary)**
- Official pricing (Oct 2026): **Free for clusters ≤5 nodes** (all features). **Personal $79/yr** ($7/mo billed yearly; 1 user, 2 devices). **Team $59/seat/yr** ($5/seat/mo) with SSO license management. 15-day trial for bigger clusters. "Every feature, on every plan": multi-cluster view, aggregated logs, pod/node metrics, port forwarding, YAML editor, GitOps, k9s-style keyboard navigation. — [Aptakube pricing](https://aptakube.com/pricing)
- Built on **Tauri**; the developer rejected SwiftUI because it would be macOS-only. Uses the existing kubeconfig and needs nothing installed in the cluster. — [Aptabase blog](https://aptabase.com/blog/why-chose-to-build-on-tauri-instead-electron), [VirtualizationHowTo, Dec 2024](https://www.virtualizationhowto.com/2024/12/aptakube-is-the-best-lens-alternative-right-now/)
- Dec 2024 pricing ($9/mo, $99/yr perpetual, $7/seat) is superseded by the page above. — [VirtualizationHowTo](https://www.virtualizationhowto.com/2024/12/aptakube-is-the-best-lens-alternative-right-now/) (pre-2025, outdated)

**kftray (Rust + Tauri + Ratatui; GPL-3.0)**
- A "kubectl port-forward manager and reverse tunnel (ngrok-like)". Ships a tray desktop app (kftray) and a TUI (kftui) on a shared Rust backend. Runs multiple forwards across contexts **without kubectl**, supports TCP and **UDP**, can expose local services into the cluster, **inspects and replays HTTP traffic**, **shares configs via GitHub**, and auto-reconnects on pod restart. 1.6k stars. — [GitHub kftray](https://github.com/hcavarsan/kftray)

**Others**
- **Monokle** (Kubeshop, Electron, MIT): maintainers say they "are not able to maintain or evolve Monokle". Last release v2.4.8 (May 16, 2024). — [AlternativeTo](https://alternativeto.net/software/monokle/about), [dev.co](https://dev.co/devops/open-source/monokle)
- **Octant** (VMware): repo archived **Jan 19, 2023** ("VMware has ended active development"). — [GitHub vmware-archive/octant](https://github.com/vmware-archive/octant)
- **Seabird**: Go + GTK4/libadwaita, designed for GNOME. MPL-2.0 but "paid software with an unlimited free trial". Integrated terminal, manifest editor, metrics-server integration. AUR has 0.6.0. — [GitHub seabird](https://github.com/getseabird/seabird), [AUR](https://aur.archlinux.org/packages/seabird-bin)

### Inferences
- The day-to-day core that every surviving tool shares: multi-cluster/context switching from kubeconfig, a resource list with YAML view/edit, streaming and aggregated logs, exec shell, port-forward that auto-reconnects (Headlamp 0.46 and kftray both treat this as a headline), metrics, Helm releases and CRD browsing. AI assistants and MCP servers are the 2026 add-on (Lens Plus/Pro, Headlamp plugin).
- kftray's "port-forward configs shared via GitHub" closely matches /dev/all's "connections as files in git". A per-project `forwards.yaml` that opens tunnels and pre-fills HTTP-client base URLs is a strong synergy.
- Aptakube's pricing (free ≤5 nodes, $79/yr) is a useful reference for a paid tier. A ≤5-node free limit means local kind/minikube/k3d clusters are effectively free, matching a dev-focused, local-first audience.
- Freelens bundles kubectl, Helm and auth-plugin wrappers. Even Electron-era GUIs still depend on external binaries for auth and Helm (Section 8).

### Gaps
- Lens Prism (AI) and the lenshq.io rebrand timeline were not confirmed.
- Exact k9s and Headlamp release dates beyond those listed are not captured. Headlamp desktop's packaging (likely Electron) is not stated on the repo page.
- Direct r/kubernetes and HN threads on Lens and Freelens were not reachable (search budget exhausted).

---

## 5. Docker and container desktop tools; what's realistic on the Docker Engine API

### Takeaway
Docker Desktop is free only for personal use, education, non-commercial OSS, and companies with <250 employees AND <$10M revenue. Paid plans run $9–$24/user/mo. OrbStack (macOS, paid for commercial use), Podman Desktop and Rancher Desktop (both free/OSS, Electron) are the main alternatives. The features devs use daily (container list, logs, stats, exec, restart/remove, compose-project grouping, prune) are covered by tools like Lazydocker and Dozzle, and all map onto the Docker Engine REST API, which `bollard` wraps in Rust for both Docker and Podman.

### Cited Findings
- **Docker plans** (official, fetched Oct 2026): Personal $0. **Pro $9/user/mo annual or $11 monthly**. **Team $15 annual or $16 monthly** (up to 100 users). **Business $24** (SSO, SCIM, Hardened Desktop, ECI, registry/image access management). Legacy plans had a Dec 10, 2024 cutoff. — [Docker pricing](https://www.docker.com/pricing/)
- **Docker Desktop license**: free for small businesses (<250 employees **and** <$10M revenue), personal use, education and non-commercial OSS. Paid for larger organizations and government. — [Docker Desktop license docs](https://docs.docker.com/subscription/desktop-license/)
- The 2021 launch prices were Pro $5, Team $7, Business $21. — [TechTarget](https://www.techtarget.com/it-infrastructure/news/252506055/Docker-Desktop-pricing-changes-in-bid-to-boost-revenue) (pre-2025, outdated)
- **OrbStack** (macOS): a license per user is required for freelance, business, commercial, non-profit or government use, or for anyone earning >$10k/yr from work involving OrbStack. Includes a 30-day Pro trial, after which the Free tier is personal-only. Up to 5 devices per user. — [OrbStack licensing](https://docs.orbstack.dev/licensing) The price of about $8/user/mo billed annually comes from aggregators (unverified). — [Toolradar](https://toolradar.com/tools/orbstack/pricing)
- **Podman Desktop**: free and OSS. Container and pod management, Kind/Minikube Kubernetes, extensions, GPU acceleration for local AI model containers, Docker migration guide, multi-arch builds, air-gapped install. — [podman-desktop.io](https://podman-desktop.io/) It is Apache-2.0 and daemonless, and Compose support is described as imperfect. — [Better Stack comparison](https://betterstack.com/community/comparisons/docker-desktop-alternative/) (secondary)
- **Rancher Desktop**: Apache-2.0. "An Electron application mainly written in TypeScript" with a Go CLI `rdctl`. 7.4k stars. — [GitHub rancher-desktop](https://github.com/rancher-sandbox/rancher-desktop) Bundles k3s Kubernetes. — [Better Stack](https://betterstack.com/community/comparisons/docker-desktop-alternative/) (secondary)
- **Portainer**: CE is OSS. Business Edition is free on up to 3 nodes with a license key. Paid prices conflict ($1,045/yr Starter, $2,095/yr Scale per one blog). — [OneUptime, Mar 2026](https://oneuptime.com/blog/post/2026-03-20-portainer-ce-vs-business/markdown), [iTechGuides](https://www.itechguides.com/portainer-pricing-is-portainer-free-in-2026/)
- **Lazydocker** (Go/gocui TUI, MIT, 53.2k stars) handles day-to-day tasks: overall state of Docker or Compose, container/service logs, ASCII metric graphs, attach, restart/remove/rebuild, image layer view, prune containers/images/volumes, and custom commands. — [GitHub lazydocker](https://github.com/jesseduffield/lazydocker)
- **Dozzle** (Go backend + Vue, MIT, 14.7k stars, ~7 MB `FROM scratch` image) is a real-time container log viewer for Docker, Swarm and K8s. It has agent mode for multi-host, **SQL querying of logs**, fuzzy and regex search, split screen, live CPU/memory stats, and multi-user auth. It does not store logs. — [GitHub dozzle](https://github.com/amir20/dozzle)
- **Docker Engine API**: the latest reference is **v1.56**, though the version matrix lists 1.55 as the max for Docker 29.8. The API is backward-compatible, with client/daemon version negotiation, path-pinned versions (`/v1.56/containers/`), and a `DOCKER_API_VERSION` override. The minimum supported version is 1.40. — [Docker Engine API docs](https://docs.docker.com/reference/api/engine/)
- **bollard** 0.21.1 (Aug 16, 2026) supports Docker and Podman, with automatic socket discovery for rootless Podman. Its API target tracks the moby release at publish time. — [crates.io bollard](https://crates.io/api/v1/crates/bollard), [docs.rs bollard 0.21.1](https://docs.rs/crate/bollard/0.21.1)
- User complaint: on Windows, Docker Desktop holds on to memory after containers stop and starves other processes. Suggested alternatives were Rancher Desktop, Podman on WSL2, and Colima plus Portainer (Lemmy thread, about 2025). — [lemm.ee thread](https://lemm.ee/post/5636453/2825450)

### Inferences
- **Realistic Docker viewer on the Engine API via bollard** (no CLI needed):
  - List containers, images, volumes and networks, grouped by the `com.docker.compose.project` label.
  - Inspect a container and show its env, ports and mounts.
  - Stream logs (follow, timestamps, since) into the /dev/all log viewer.
  - Stream live stats (CPU/mem).
  - Exec with a TTY into an embedded terminal.
  - Start, stop, restart and remove containers, and prune.
  - Show the events stream.
  - Pull images with progress.
- **Harder or out of scope:** Compose up/down/build orchestration (Compose is a client-side tool, so shelling out to `docker compose` is pragmatic). BuildKit builds are also in this bucket, though bollard 0.17+ has some BuildKit pieces.
- The app should not try to replace Docker Desktop, OrbStack or Podman (the VM/runtime layer). It should connect to whichever socket exists, as bollard's Podman socket discovery already does. That avoids the licensing debate entirely.
- Dozzle's "SQL over logs" shows demand for querying container logs, which fits the log viewer plus an embedded DuckDB (Section 7).

### Gaps
- Docker Desktop 2026 changes (for example, Docker Model Runner and AI features) and recent Docker Desktop licensing changes were not researched (search budget).
- OrbStack's official price was not confirmed on orbstack.dev/pricing.
- Podman Desktop's license, version and Electron stack were not confirmed on its official page.
- Windows named-pipe and SSH connection details were not captured from Docker docs.

---

## 6. GCP and cloud: lightweight GUIs, emulators, most valuable subset

### Takeaway
GCP has no first-party lightweight desktop GUI: there is Cloud Console, `gcloud`, and IDE plugins (Cloud Code). The Pub/Sub emulator has **no UI at all**. Only small community web UIs exist and there is no native desktop one, which is a clear gap. The Firebase Emulator Suite has its own web UI (port 4000). LocalStack (AWS) **now requires an auth token and archived its OSS repo in March 2026**, which upset free users. The best small GCP subset for devs is likely: Pub/Sub (emulator + real), Cloud Storage browser, Cloud Logging tail, Secret Manager (read into local secrets), and Cloud SQL via the Auth Proxy.

### Cited Findings
- **Pub/Sub emulator**: install with `gcloud components install pubsub-emulator` (or the gcloud Docker image) and start with `gcloud beta emulators pubsub start --project=…`. Default port 8085. Clients use `PUBSUB_EMULATOR_HOST`. **"The emulator doesn't support the Google Cloud console or `gcloud pubsub` commands"**, so you interact only through code. **C#, Java and Rust need code changes** to target the emulator. Supported: publish, push/pull, ordering, replay, dead-letter, retry, Avro schemas, filtering. Limitations: no UpdateTopic/UpdateSnapshot, no IAM, indefinite retention, no proto schemas, BigQuery subscriptions don't deliver, no SMT transforms. — [Pub/Sub emulator docs](https://docs.cloud.google.com/pubsub/docs/emulator)
- Community Pub/Sub emulator UIs are **all web-based**: NeoScript/pubsub-emulator-ui; indritvaka/pubsub-emulator-console (MIT; publish, peek, pull, ack/nack, purge; no auth); ClickAndMortar/GoPubSub; lukastosic/pubsubmanager (about 4 years stale); DennisVis/pubsubui. No native desktop app was found. — [Google Developers forum](https://discuss.google.dev/t/pubsub-emulator-ui/145303), [Docker Hub console](https://hub.docker.com/r/indritvaka/pubsub-emulator-console), [GoPubSub](https://pkg.go.dev/github.com/ClickAndMortar/GoPubSub), [pubsubui](https://github.com/DennisVis/pubsubui)
- **Firebase Local Emulator Suite**: Firestore, Realtime Database, Cloud Storage, Authentication, Hosting, Cloud Functions (beta), Pub/Sub (beta), Extensions (beta), with "a rich user interface". — [Firebase docs](https://firebase.google.com/docs/emulator-suite) The Emulator UI is a web dashboard on **port 4000**. Firestore and RTDB emulators need Java JDK 11+. — [tessl firebase-tools docs](https://tessl.io/registry/tessl/npm-firebase-tools/files/docs/emulators.md), [rapidevelopers](https://www.rapidevelopers.com/md/firebase-tutorial/how-to-use-firebase-emulator-suite) (secondary)
- **LocalStack (AWS)**: as of **2026.03.0** an auth token is required, Community and Pro images merged into one, and services depend on account entitlements. `LOCALSTACK_ACKNOWLEDGE_ACCOUNT_REQUIREMENT=1` deferred this only until Apr 6, 2026. State from earlier versions is incompatible. — [LocalStack 2026.03.0 release](https://blog.localstack.cloud/localstack-for-aws-release-2026-03-0/), [LocalStack single-image post](https://blog.localstack.cloud/localstack-single-image-next-steps/)
- The localstack/localstack GitHub repo was **archived Mar 23, 2026** (65.1k stars). It points to a free **Hobby plan for non-commercial use** and lists GUIs: LocalStack Web Application, **LocalStack Desktop**, and a Docker Extension. — [GitHub localstack](https://github.com/localstack/localstack) Commercial seats are reportedly $39/mo annual or $45 monthly (aggregator, unverified). — [agentdeals](https://agentdeals.dev/vendor/localstack)
- **Cloud SQL Auth Proxy**: a standalone binary (or Docker image) giving TLS 1.3, IAM-authorized access without authorized networks. Optionally refreshes OAuth tokens for IAM DB auth. No connection pooling. Language connectors exist for **Go, Java, Python**; **no Rust connector** is mentioned. — [Cloud SQL Auth Proxy docs](https://docs.cloud.google.com/sql/docs/postgres/sql-proxy)
- **Cloud Code for VS Code**: GKE app dev and YAML authoring, a Cloud Run explorer with logs and local emulator, **Secret Manager** create/view/update, an API browser, and Compute Engine VM browse/SSH/file transfer. Gemini Code Assist is now a separate plugin, and its individual tiers stopped serving on **Jun 18, 2026** (users pointed to "Antigravity"). — [Cloud Code features](https://docs.cloud.google.com/code/docs/vscode/features)
- Paid multi-tool coverage of cloud: DBeaver Ultimate ($510/yr) adds Cloud Explorer for AWS/GCP/Azure and cloud storage. — [DBeaver editions](https://dbeaver.com/edition/) Lens Plus adds EKS/AKS/GKE integration. — [Lens pricing](https://lenshq.io/pricing)
- **Official Google Cloud Rust SDK**: announced Sept 2025 covering 140+ APIs. — [Google Cloud blog](https://cloud.google.com/blog/topics/developers-practitioners/now-available-rust-sdk-for-google-cloud) The overview (updated 2026-10-20) lists Storage, Vertex AI, Secret Manager "and over 150 other services", on Tokio async. — [Rust on Google Cloud overview](https://docs.cloud.google.com/rust/docs/overview)

### Inferences
- **Highest-value small GCP subset** for a local-first dev toolkit:
  1. **Pub/Sub**: topic and subscription browser, publish (with an LLM/mock-generated payload), and pull/peek into the log viewer. It works against both the emulator and real projects, and fills the "emulator has no UI" gap.
  2. **Cloud Storage browser**: list, upload, download and preview objects. It can also seed DB fixtures from GCS objects.
  3. **Cloud Logging tail**: a filter query streamed into the log viewer.
  4. **Secret Manager**: read-only fetch into the local secrets store, never committed. This matches the "secrets local only" rule.
  5. **Cloud SQL**: spawn and manage `cloud-sql-proxy` per connection, then use the normal SQL client.
- The same pattern (emulator-first UI plus the real service via ADC) generalizes to the Firebase emulators and to LocalStack. LocalStack's 2026 auth-token change makes "works with any emulator, no account" a selling point.

### Gaps
- No search on third-party GCP desktop GUIs (for example, GCS support in Cyberduck or gcloud TUIs) was possible after the search budget ran out.
- Firebase Emulator UI per-panel features were not confirmed from official docs.
- Cloud Logging tail API support in the Rust SDK was not verified.
- The "LocalEmu" successor fork claim was not verified. — [Medium/PlainEnglish](https://aws.plainenglish.io/meet-localemu-the-free-successor-to-localstack-b9755ba6c91e)

---

## 7. Rust libraries: maturity and gaps

### Takeaway
Every needed protocol has an actively maintained Rust crate as of Oct 2026. The main gaps are:
- sqlx has no SQL Server support and is oriented to compile-time-checked queries. A GUI needs per-driver crates (`tokio-postgres`, `mysql_async`, `rusqlite`, `tiberius`) for full type and metadata fidelity.
- `fred` hasn't released since Feb 2025, while `redis-rs` hit 1.x.
- Google's Rust SDK is GA-ish but has streams behind an unstable flag and no Cloud SQL connector.
- kube-rs fully supports exec, attach, port-forward, log streaming and exec credential plugins.

### Cited Findings
(versions and dates from the crates.io API, fetched 2026-10-21)

| Crate | Latest | Date | Downloads (total / recent) | Notes |
|---|---|---|---|---|
| `sqlx` | 0.9.0 | 2026-05-21 | 169.6M / 48.1M | PG, MySQL, SQLite only; `Any` runtime driver; `QueryBuilder`, `raw_sql` — [crates.io](https://crates.io/api/v1/crates/sqlx), [docs.rs](https://docs.rs/sqlx/latest/sqlx/) |
| `tokio-postgres` | 0.7.18 | 2026-06-12 | 77.8M / 21.7M | [crates.io](https://crates.io/api/v1/crates/tokio-postgres) |
| `mysql_async` | 0.37.1 | 2026-09-01 | 8.2M / 1.7M | 0.37.0 yanked — [crates.io](https://crates.io/api/v1/crates/mysql_async) |
| `rusqlite` | 0.40.2 | 2026-08-08 | 131.0M / 46.7M | [crates.io](https://crates.io/api/v1/crates/rusqlite) |
| `tiberius` (SQL Server, Prisma) | 0.13.0 | 2026-09-25 | 5.6M / 0.84M | first release since Jul 2024 — [crates.io](https://crates.io/api/v1/crates/tiberius) |
| `duckdb` | 1.10506.1 | 2026-10-16 | 5.0M / 2.1M | versioning jumped from 1.4.5 to 1.10504.0 in Jun 2026 — [crates.io](https://crates.io/api/v1/crates/duckdb) |
| `redis` (redis-rs) | 1.7.1 | 2026-09-25 | 117.8M / 30.8M | rapid 1.x cadence (1.4.1 Jul to 1.7.1 Sep 2026) — [crates.io](https://crates.io/api/v1/crates/redis) |
| `fred` | 10.1.0 | 2025-02-27 | 10.2M / 2.5M | "async client for Redis and Valkey"; no release in about 20 months — [crates.io](https://crates.io/api/v1/crates/fred) |
| `mongodb` (official) | 3.9.1 | 2026-09-10 | 19.7M / 5.1M | monthly releases — [crates.io](https://crates.io/api/v1/crates/mongodb) |
| `kube` (kube-rs) | 4.2.0 | 2026-07-22 | 83.5M / 22.4M | 3.0.0 Jan 2026, 4.0.0 Jun 2026 — [crates.io](https://crates.io/api/v1/crates/kube) |
| `bollard` | 0.21.1 | 2026-08-16 | 60.4M / 18.9M | Docker + Podman — [crates.io](https://crates.io/api/v1/crates/bollard) |
| `google-cloud-pubsub` (official) | 1.6.0 | 2026-09-30 | 8.0M | googleapis/google-cloud-rust — [crates.io](https://crates.io/api/v1/crates/google-cloud-pubsub) |
| `russh` (pure Rust SSH client/server) | 0.64.1 | 2026-10-05 | 8.8M / 3.7M | frequent 0.x releases — [crates.io](https://crates.io/api/v1/crates/russh) |
| `ssh2` (libssh2 bindings) | 0.9.6 | 2026-06-30 | 8.3M / 1.3M | previous release Feb 2025 — [crates.io](https://crates.io/api/v1/crates/ssh2) |

- **sqlx**: docs describe support for "PostgreSQL, MySQL, and SQLite". MSSQL is not listed. The `Any` driver selects the backend at runtime from the URL and is marked "SEE DOCUMENTATION BEFORE USE". `query*` functions run without compile-time checks, and `Column` and `TypeInfo` give limited column metadata. The docs.rs page shows 0.9.0 dated 30 Sept 2026, which conflicts with crates.io's 2026-05-21; this may be a docs rebuild date. — [docs.rs sqlx](https://docs.rs/sqlx/latest/sqlx/)
- **kube-rs**: a CNCF **Sandbox** project modeled on client-go, with a controller runtime and `#[derive(CustomResource)]`. Tested on **Kubernetes v1.32+**, MSRV 1.89, rustls/ring by default. — [GitHub kube-rs](https://github.com/kube-rs/kube) Features include `ws`, `oauth`, `oidc`, `socks5`, `http-proxy`, `kubelet-debug`, `runtime`, `derive` and more. — [crates.io kube](https://crates.io/api/v1/crates/kube)
- kube-rs `Api` methods: `exec(name, command, &AttachParams) -> AttachedProcess`, `attach(...)` and `portforward(name, &[u16]) -> Portforwarder` (all need the **`ws`** feature), plus `log_stream(name, &LogParams) -> impl AsyncBufRead` and `logs()`. — [docs.rs kube Api](https://docs.rs/kube/latest/kube/api/struct.Api.html)
- kube-rs `ExecConfig` models kubeconfig **exec credential plugins** (command, args, env, api_version, interactive_mode, `provide_cluster_info` via the `KUBERNETES_EXEC_INFO` env var, install_hint). Env merging with the host environment is marked TODO. — [docs.rs ExecConfig](https://docs.rs/kube/latest/kube/config/struct.ExecConfig.html)
- **bollard** auto-discovers rootless Podman sockets and treats Podman as first-class. Older versions pinned specific moby API versions (0.16 = API 1.44, 0.17 = 1.45 with BuildKit secrets/ssh-forward). — [docs.rs bollard 0.21.1](https://docs.rs/crate/bollard/0.21.1), [docs.rs bollard-next](https://docs.rs/bollard-next/)
- **google-cloud-rust**: Apache-2.0, **MSRV Rust 1.91**. Many crates are generated, so breaking changes cause major bumps. **Async streams are behind an `unstable-streams` feature**. — [GitHub google-cloud-rust](https://github.com/googleapis/google-cloud-rust) The storage crate says its types are "stable and not expected to change". The `google-cloud-storage` name was donated by the former community crate maintainer, whose crate continues as `gcloud-storage`. — [docs.rs google-cloud-storage](https://docs.rs/crate/google-cloud-storage/1.17.0/source/README.md) A 1.0/GA milestone was reported Sept 2025 (secondary). — [x-cmd](https://www.x-cmd.com/blog/250911)
- **fred** still advertises RESP2/RESP3, cluster, sentinel, pub/sub and keyspace events, TLS, reconnection, pooling, client tracking and built-in mocking. No archive notice, 534 stars. — [GitHub fred.rs](https://github.com/aembke/fred.rs)
- **Rust/Tauri precedents**: kftray (Rust backend + Tauri + Ratatui) does K8s port-forwarding without kubectl. — [GitHub kftray](https://github.com/hcavarsan/kftray) Aptakube is a commercial Tauri K8s client. — [Aptabase blog](https://aptabase.com/blog/why-chose-to-build-on-tauri-instead-electron)

### Inferences
- **SQL layer recommendation**: use per-engine native drivers behind an internal trait (`tokio-postgres` for PG and Cockroach, `mysql_async` for MySQL/MariaDB, `rusqlite` for SQLite, `tiberius` for MSSQL, `duckdb` for local analytics and CSV/Parquet). A GUI needs arbitrary user SQL, full column metadata, multiple result sets, cancellation and server notices, which are areas where sqlx's compile-time focus and `Any` driver are weakest. sqlx remains fine for the app's own internal SQLite storage.
- **Redis**: prefer `redis` (redis-rs 1.x, very active) over `fred` given fred's 20-month release gap. fred's RESP3, client-tracking and sentinel ergonomics are still attractive, so its health should be watched.
- **Embedded DuckDB** is a cheap multiplier. It can query exported result sets, log files (Dozzle-style "SQL over logs") and API responses ("seed a DB from an API response").
- **SSH tunnels**: `russh` (pure Rust, very active) avoids a C libssh2 dependency and cross-compile pain. `ssh2` is slower-moving. Either way, ~/.ssh/config, agent and ProxyJump support has to be implemented in the app (not verified as provided by either crate).
- tiberius' 2-year release gap followed by 0.13.0 in Sept 2026 suggests MSSQL support is the riskiest SQL driver dependency.

### Gaps
- Whether `russh` or `ssh2` parse ~/.ssh/config, ProxyJump and agent forwarding natively was not verified.
- Whether the `google-cloud-logging` (tail) and `google-cloud-secretmanager` crates are GA was not verified individually.
- The mapping from `duckdb` crate 1.105xx versions to DuckDB engine versions was not confirmed.
- How kube-rs handles the legacy `auth-provider: gcp` path vs. exec plugins (feature `oauth`) was not detailed.

---

## 8. Shell out to kubectl / gcloud / docker CLIs vs. native libraries; auth-plugin issues

### Takeaway
Use a **hybrid**: native libraries (kube-rs, bollard, official Google crates) for all data-plane UX (lists, watches, logs, exec, port-forward). Accept that **auth still means running external binaries**: kubeconfig exec credential plugins (`gke-gcloud-auth-plugin`, `kubelogin`, `aws`) must be spawned even by kube-rs. Shell out for complex client-side tools (Helm, `docker compose`, `cloud-sql-proxy`). Most real-world failures are PATH and environment problems with those plugins, so invest in plugin discovery and diagnostics.

### Cited Findings
- GKE's `gke-gcloud-auth-plugin` "uses the client-go Credential Plugins framework" to provide tokens. The legacy built-in GCP auth provider was deprecated. — [GKE auth plugin deprecation notice](https://docs.cloud.google.com/kubernetes-engine/docs/deprecations/auth-plugin)
- The common failure is "exec: executable gke-gcloud-auth-plugin not found", caused by the plugin not being installed or on the expected path. — [Gurubase](https://gurubase.io/g/kubernetes/exec-executable-gke-gcloud-auth-plugin-not-found-on-kubectl-get-pods), [Kubernetes discuss](https://discuss.kubernetes.io/t/unable-to-connect-to-the-server-getting-credentials-exec-executable-gke-gcloud-auth-plugin-not-found/23004)
- One user found the absolute Homebrew path in kubeconfig worked but the bare command name failed in some contexts. A GUI-launched app does not inherit the shell PATH. — [Kubernetes discuss](https://discuss.kubernetes.io/t/unable-to-connect-to-the-server-getting-credentials-exec-executable-gke-gcloud-auth-plugin-not-found/23004)
- On Windows, a GUI client (Lens) failed because the plugin existed only inside **WSL**. The fix was routing the exec command through `wsl.exe`. — [TabNews](https://www.tabnews.com.br/danieldia/usando-o-gke-gcloud-auth-plugin-do-wsl-dentro-do-lens)
- kube-rs implements exec credential plugins (`ExecConfig`: command, args, env, `provide_cluster_info`, `install_hint`). `drop_env` is not configurable from kubeconfig and env merging is still TODO. — [docs.rs ExecConfig](https://docs.rs/kube/latest/kube/config/struct.ExecConfig.html)
- Freelens **bundles kubectl** (and Helm in Flatpak) and ships host wrappers for `aws`, `doctl`, `gke-gcloud-auth-plugin` and `kubelogin` because the Flatpak sandbox hides host binaries. It falls back to bundled kubectl for old clusters. — [GitHub freelens](https://github.com/freelensapp/freelens)
- Headlamp 0.46.0 lets desktop builds define "an explicit list of approved commands", a security control over which external commands, such as exec plugins, may run. — [Headlamp releases](https://github.com/kubernetes-sigs/headlamp/releases)
- kftray runs port-forwards "without kubectl" from a Rust backend. — [GitHub kftray](https://github.com/hcavarsan/kftray) Aptakube needs only the existing kubeconfig. — [Aptakube](https://aptakube.com/pricing)
- The Pub/Sub emulator requires Rust code changes rather than only `PUBSUB_EMULATOR_HOST`. — [Pub/Sub emulator docs](https://docs.cloud.google.com/pubsub/docs/emulator) Cloud SQL Auth Proxy offers Go, Java and Python connectors but no Rust one. — [Cloud SQL Auth Proxy docs](https://docs.cloud.google.com/sql/docs/postgres/sql-proxy)
- The Docker Engine API is versioned with negotiation and backward compatibility (min 1.40). — [Docker Engine API](https://docs.docker.com/reference/api/engine/) Lazydocker likewise uses the API (requires API ≥1.24). — [GitHub lazydocker](https://github.com/jesseduffield/lazydocker)

### Inferences
- **Pros of native libs**: structured data (no output parsing), streaming watches and logs, reliable cancellation, no version skew with the user's kubectl, a single binary, and works on machines without the CLIs (kftray and Aptakube prove the UX).
- **Cons of native libs**: auth plugins still require spawning processes. Some ecosystems are client-side tools with no API (Helm 4, `docker compose`, `cloud-sql-proxy`). Edge-case kubeconfig features (env merging, legacy auth providers) may lag client-go.
- **Recommended design**:
  - Resolve a "login-shell PATH" at startup on macOS and Linux to fix GUI PATH issues. On Windows, offer a WSL bridge for exec plugins.
  - Show `install_hint` from kubeconfig when a plugin is missing, plus a "test connection" with the plugin's stderr.
  - Keep an allow-list of exec-plugin commands, as Headlamp 0.46 does.
  - Treat `helm`, `docker compose` and `cloud-sql-proxy` as optional managed sidecar binaries, detected or downloaded per project.
- Storing kubeconfig *references* (context names, file paths) in the project git repo, rather than the kubeconfig itself, keeps credentials local, consistent with /dev/all's secrets rule.

### Gaps
- No direct evidence on how kube-rs behaves when a plugin needs interactive input (`interactive_mode`) inside a GUI without a TTY.
- Helm SDK equivalents in Rust were not researched (likely none; Helm is Go).
- No benchmarks comparing CLI shell-out vs. native-library latency.

---

## 9. Common user complaints and wishes (Reddit/HN/GitHub)

### Takeaway
Complaints cluster around:
1. **Resource bloat** in Java and Electron apps (DBeaver RAM and startup, Electron memory, Docker Desktop memory).
2. **Licensing and paywall shifts** (Lens EULA and closed source, Docker Desktop for businesses, LocalStack auth token and repo archive, DBeaver moving NoSQL and other features to PRO, TablePlus free-tier limits).
3. **Cloud-only AI** that ships schema to third parties.
4. **Abandonment** (Robo 3T, Octant, Monokle, Kubernetes Dashboard, Azure Data Studio, Arctype, RESP.app).

Users want fast native-feeling apps, honest free tiers, local and private AI, and tools that keep working without accounts.

### Cited Findings
- DBeaver reportedly "routinely eats 1–2 GB of RAM at idle", with 8–15 s cold launches. Users feel the free edition is shrinking as NoSQL, visual query builder and schema compare moved to Pro. A quoted Reddit commenter said "Most 'DBeaver alternatives' are just the same idea with different UI tradeoffs." These come from vendor and secondary comparison blogs and could not be attributed to a single one. — [DbVis](https://www.dbvis.com/thetable/best-dbeaver-alternatives-of-2025/), [FuturePicker](https://futurepicker.com/?p=1927), [devtoollab](https://devtoollab.com/blog/best-sql-clients)
- DBeaver's own edition matrix confirms MongoDB, Redis, cloud and Visual Query Builder are PRO-only. — [dbeaver.io](https://dbeaver.io/)
- TablePlus: praised for speed, criticized for its free-tier limits (2 tabs, 2 windows) and per-device pricing. — [TablePlus pricing](https://tableplus.com/pricing), [devtoollab](https://devtoollab.com/blog/best-sql-clients)
- Beekeeper: Electron memory overhead, and AI and cloud features need paid tiers. — [devtoollab](https://devtoollab.com/blog/best-sql-clients), [Beekeeper pricing](https://www.beekeeperstudio.io/pricing)
- Lens: forced sign-in and payment for large orgs, features removed from OpenLens, and source closed. This drove Freelens adoption. — [The New Stack](https://thenewstack.io/the-open-source-ethos-why-lens-made-a-mistake/) (pre-2025), [DevGenius](https://blog.devgenius.io/i-recommended-openlens-now-its-time-to-move-to-freelens-ae1a86773f80)
- Docker Desktop: memory not released on Windows. Users move to Rancher Desktop, Podman on WSL2, or Colima plus Portainer. — [lemm.ee](https://lemm.ee/post/5636453/2825450)
- LocalStack: the 2026 auth-token requirement and repo archive. A third-party site labels the free tier "risky". — [LocalStack blog](https://blog.localstack.cloud/localstack-single-image-next-steps/), [agentdeals](https://agentdeals.dev/vendor/localstack)
- Compass NL query sends prompts and schema to Microsoft/OpenAI and is "experimental… may give inaccurate results". — [Compass NL docs](https://www.mongodb.com/docs/compass/query-with-natural-language/) Studio 3T's AI has "no on-premises model option". — [Studio 3T buy](https://studio3t.com/buy/)
- Azure Data Studio's retirement left users without feature parity and needing Windows-only SSMS for some tasks. — [DevClass](https://www.devclass.com/databases/2025/02/11/microsoft-drops-azure-data-studio-in-favour-of-visual-studio-code-extension-despite-missing-features/1629157), [Directions on Microsoft](https://www.directionsonmicrosoft.com/reports/azure-data-studio-is-retiring-what-you-need-to-do-and-when/)
- GKE auth-plugin PATH and WSL failures in GUI clients. — [TabNews](https://www.tabnews.com.br/danieldia/usando-o-gke-gcloud-auth-plugin-do-wsl-dentro-do-lens), [Kubernetes discuss](https://discuss.kubernetes.io/t/unable-to-connect-to-the-server-getting-credentials-exec-executable-gke-gcloud-auth-plugin-not-found/23004)
- Wishes visible in 2026 shipping features:
  - Port-forwards that survive pod restarts (Headlamp 0.46, kftray). — [Headlamp releases](https://github.com/kubernetes-sigs/headlamp/releases), [GitHub kftray](https://github.com/hcavarsan/kftray)
  - Lower desktop memory (Headlamp 0.45, −60 MiB). — [Headlamp releases](https://github.com/kubernetes-sigs/headlamp/releases)
  - Signed binaries to avoid AV false positives (Freelens 1.10.2). — [Freelens releases](https://github.com/freelensapp/freelens/releases)
  - Loading huge Redis keyspaces without crashing (ARDM). — [GitHub ARDM](https://github.com/qishibo/AnotherRedisDesktopManager)
  - Read-only and safe modes for production (TablePlus Safe mode, k9s `--readonly`, ARDM read-only, Sequel Ace's read-only MCP). — [tableplus.com](https://tableplus.com/), [GitHub k9s](https://github.com/derailed/k9s), [GitHub Sequel-Ace](https://github.com/Sequel-Ace/Sequel-Ace)

### Inferences
- /dev/all's positioning writes itself from these complaints:
  - A small Tauri/Rust binary (vs. Electron/JVM bloat).
  - Local-first, account-free, git-stored connections and queries (vs. Lens and LocalStack sign-in walls).
  - A **local LLM** for NL→SQL/Mongo/Redis/PromQL that never sends schema off-device (vs. Compass, Studio 3T, DBeaver and Beekeeper cloud AI).
  - Production-safety defaults: read-only connection flag in the git file, confirm-on-write, highlighted prod contexts.
- Cross-tool synergies no competitor offers, because each is single-domain:
  - Pod or container logs into the shared log viewer with SQL/DuckDB querying.
  - Port-forward feeding an HTTP-client environment.
  - Mock-data generator to INSERT, Redis HSET or Mongo insertMany.
  - API response to table seed.
  - Pub/Sub emulator publish with LLM-generated payloads.
- Abandonment history (Octant, Monokle, Robo 3T, Dashboard, ADS, Arctype, RESP.app) suggests users value **open formats** (plain YAML/SQL files) that outlive the tool, which supports the git-file design.

### Gaps
- **Direct Reddit and HN threads were largely not retrieved**: the shared web-search budget was exhausted mid-research, and most complaint evidence comes from vendor or secondary comparison blogs. Quotes attributed to "Reddit commenters" are second-hand.
- No quantitative survey data (for example, Stack Overflow Developer Survey 2025/2026 DB-tool usage) was gathered.
- No collected sentiment for Headlamp desktop, Aptakube, Podman Desktop or OrbStack from user forums.

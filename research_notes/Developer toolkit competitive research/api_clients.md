# API Client / HTTP Client Landscape (as of October 2026)

Research notes for "/dev/all" (local-first, git-backed, Rust + Tauri dev toolkit). Collected 2026-10-21.

Method note: the shared web-search budget ran out partway through. Later facts come from direct fetches of official pages (vendor sites, docs, GitHub, Hacker News). Some items could not be verified and are listed under Gaps. Vendor-written comparisons, especially Apidog's many "Postman alternative" posts, are labeled as vendor claims. Anything dated before 2025 is tagged **[pre-2025, possibly outdated]**.

---

## Q1. What features does each tool have (protocols, auth, cookies, proxies, environments, scripting, tests, runners, CLI, codegen, import/export, chaining, visualizers, history)?

### Takeaway
By late 2026 the major GUI clients (Postman 12, Insomnia 13, Bruno 4, Yaak 2026.x, Hoppscotch 2026.x, Kreya) all cover REST, GraphQL, WebSocket and gRPC. Most also ship mock servers, a CLI runner and some AI/MCP features. The real differences now are:
- the long-tail protocols (Socket.IO, MQTT, SSE, SOAP);
- whether scripting and response history exist at all;
- which features sit behind a paywall.

Terminal and file-based tools (Hurl, .http files, Posting, ATAC) cover a narrower set of features but are the most git-native.

### Cited Findings

**Postman (v12, released ~March 2026)**
- Postman 12 is "built from the ground up to be Git-native". It introduces a Collection 3.0 format that splits collections into individual YAML files. It also adds code-based local mock servers that run locally or in CI, a revised Postman CLI that runs the same collections, tests and mocks locally and in CI, and "Agent Mode" for AI-driven creation of collections, tests and mocks. Current version at time of article was 12.0.3. — [heise, 4 Mar 2026](https://heise.de/-11198271)
- Collection Runner automation covers HTTP, GraphQL and gRPC, with "other protocols to follow". MCP is listed as one of the protocols that can be organized in a collection. — [heise](https://heise.de/-11198271)
- The signed-out "lightweight API Client" (which replaced Scratch Pad) can send HTTP, WebSocket, gRPC and GraphQL requests, stored locally and not synced. It has no collections. — [Postman docs](https://learning.postman.com/docs/getting-started/using-api-client); [Postman help: lost requests after Scratch Pad](https://learning.postman.com/help/resolve-issues/postman-app/i-lost-my-requests-after-migrating-from-the-scratch-pad.md)
- All plans include unlimited Collection Runner use, Mock Servers, unlimited local performance testing, Local Vault, Secret Scanner and Native Git. AI credits are metered: Free 50–100/mo (the page is inconsistent), Solo 400, Team 400/user, Enterprise 800/user pooled. — [postman.com/pricing](https://www.postman.com/pricing/)

**Insomnia (Kong; v13.x in 2026)**
- 13.3.0 (23 Sep 2026) added an opt-in QuickJS sandbox for pre-request and after-response scripts, an `insomnia.sendRequest()` bridge and SSE improvements (proxy support, stream summaries), and merged document and collection workspaces. 13.3.1 (9 Oct 2026) is the latest stable and ships Inso CLI builds for Linux, macOS, Windows and Docker. — [GitHub releases](https://github.com/Kong/insomnia/releases)
- 13.2.0 (25 Aug 2026) added an MCP client (MCP OAuth, MCP workspaces), vetted npm libs (uuid, ajv) in the script sandbox, multi-file plugins, OAuth2 logs in the Console timeline and large-response handling. 13.2.0-beta.0 references WebSocket and Socket.IO, gRPC proto handling and mock-server route fixes. — [GitHub releases](https://github.com/Kong/insomnia/releases)
- The free Essentials plan includes unlimited collection runs and environments, the Inso CLI, 1,000 mock requests/month, E2EE, custom linting, plugins and "AI-powered quality-of-life features". — [insomnia.rest/pricing](https://insomnia.rest/pricing)

**Bruno (v4.x in 2026; Electron; MIT core)**
- v4.0.0 (23 Jul [2026]) added:
  - AI (chat sidebar, autocomplete, **OpenAI-compatible endpoints**);
  - split variables/secrets tabs;
  - variable data types;
  - a faster file-cache collection mount (beta);
  - YAML as the default import format;
  - multi-message WebSocket;
  - Akamai auth.
  
  [GitHub releases](https://github.com/usebruno/bruno/releases) (the page shows year only for v4.2.1; the year is inferred from ordering)
- v4.1.0 (20 Aug) added a **Mock Server**, a rich-text docs editor, global client certificates, Git remote ops and proxy config, GCP Secret Manager, multi-collection monorepo opening and a SQLite SDK. v4.2.0 (23 Sep) added gRPC scripting, environment inheritance across collections and workspaces, CLI doc generation and "Create PR in Git". v4.2.1 (30 Sep 2026) was a security release that restricted QuickJS sandbox module paths. — [GitHub releases](https://github.com/usebruno/bruno/releases)
- All tiers include HTTP, REST, GraphQL and gRPC, docs, code generation, history, an unlimited collection runner and basic Git UI (clone, diff, pull, commit). Data-driven testing and report generation are not in Open Source or Pro. — [usebruno.com/pricing](https://www.usebruno.com/pricing)
- CLI: `npm install -g @usebruno/cli`; `bru run` runs collections, folders or requests against an environment. Docker images are available for CI. — [GitHub README](https://github.com/usebruno/bruno)

**Yaak (Tauri + Rust + React; by Insomnia's original creator Greg Schier)**
- Protocols: REST, GraphQL, gRPC, WebSocket, SSE. Auth: OAuth 2.0, JWT, Basic, plus custom auth plugins. Template tags (UUIDs, timestamps). Plugin system. Imports from Postman, Insomnia, OpenAPI, Swagger and cURL. — [GitHub](https://github.com/mountain-loop/yaak)
- 2026 releases:
  - 2026.1: rewritten HTTP layer showing the full request/response flow like `curl -v`, configurable shortcuts, an experimental MCP Server plugin.
  - 2026.4: cookie editing, and settings inheritance across workspace, folder and request.
  - 2026.5: readable streaming responses for LLM APIs, and CLI import/export commands.
  - 2026.6: a "Yaak skill" for coding agents.
  - 2026.7: a new OpenAPI importer, forcing HTTP/1.1 or HTTP/2 per request, folder or workspace, and fixes for 29 security advisories in dependencies.
  
  [yaak.app changelog 2026.1.0](https://yaak.app/changelog/2026.1.0); [2026.7.0](https://yaak.app/changelog/2026.7.0); [newreleases 2026.5.0](https://newreleases.io/project/github/mountain-loop/yaak/release/v2026.5.0); [newreleases 2026.6.0](https://newreleases.io/project/github/mountain-loop/yaak/release/v2026.6.0)
- Yaak 2.0 (2025.1.1, Feb 2025) introduced Git, WebSockets and OAuth. — [yaak.app blog 2025.1.1](https://yaak.app/blog/2025.1.1); [HN 43107506](https://hn.algolia.com/api/v1/search?query=yaak&tags=story&hitsPerPage=20)
- **No pre/post-request scripting.** "Request Scripting" is the #2 feature request (64 votes, "Under Consideration"). Response history ("Save Request Data for Response History", 47 votes) is "In Progress". The author points to request chaining via template functions as the alternative to scripts. — [yaak.app/feedback](https://yaak.app/feedback); [HN Show HN thread](https://news.ycombinator.com/item?id=43185836)

**Hoppscotch (MIT; web + Tauri desktop; self-hostable; ~80.7k GitHub stars)**
- Protocols: REST (custom methods), GraphQL, WebSocket, SSE, **Socket.IO, MQTT**. Auth: None, Basic, Bearer, OAuth 2.0, OIDC/PKCE. JS pre-request scripts and post-request tests. Code snippets for 10+ languages. History. Proxy mode for CORS. Hoppscotch CLI. Import/export via files and GitHub gists. cURL import. — [GitHub](https://github.com/hoppscotch/hoppscotch)
- The free plan includes a collection runner, pre-request scripts and tests. The Business plan ($6) adds API mocking, API documentation publishing and **MCP server support**. Enterprise adds "AI agent features". The site banner announces "Enterprise Cloud + AI Assistant" (v2026.9.0). — [hoppscotch.com/pricing](https://hoppscotch.com/pricing)

**Kreya (riok GmbH; proprietary freemium)**
- Protocols: gRPC, REST, GraphQL, WebSocket, SSE. HTTP/1.1, HTTP/2 and **HTTP/3**. Transports include **Unix domain sockets and Windows named pipes**. Watches and imports OpenAPI, GraphQL schemas, Protobuf and gRPC server reflection. — [kreya.app](https://kreya.app/)
- Auth vault: OAuth 2.0, OIDC, Basic, API keys, static tokens, **mTLS, AWS SigV4, Kerberos, NTLM, JWT Profile**. — [kreya.app](https://kreya.app/)
- Other features:
  - JS scripting with npm modules;
  - **snapshot tests** that baseline responses and diff them;
  - data-driven tests;
  - a CLI with JUnit output for CI;
  - mock data generation;
  - response "data previews" (visualizers);
  - imports from Postman, Insomnia, HAR, cURL, gRPCurl and grpc-web curl.
  
  [kreya.app](https://kreya.app/)
- WebSocket support arrived in Kreya 1.16 (Jan 2025). — [Kreya blog via HN](https://kreya.app/blog/kreya-1.16-whats-new/)
- Free tier excludes scripting and testing, snapshot tests, collections, operation history (keeps "only latest request") and timing details. These are Pro features. — [kreya.app/pricing](https://kreya.app/pricing/)
- G2 lists GraphQL and SOAP as "coming soon", which conflicts with the vendor site listing GraphQL as shipped. G2 is likely stale. — [G2](https://www.g2.com/products/kreya/discuss)

**Thunder Client (VS Code / JetBrains extension)**
- Free: VS Code/JetBrains extension, **WebSocket/SSE/gRPC**, MCP Server (AI). Capped at 3 collections, 30 requests per collection and 5 imports/month. Starter ($3) adds team features and CLI & CI/CD. Enterprise ($16) adds secrets manager integration (Azure & AWS), audit logs and offline activation. Performance testing is "coming soon". — [thunderclient.com/pricing](https://www.thunderclient.com/pricing)
- Features: collections, env vars, "scriptless" GUI-based testing, Git Sync, and a CLI for requests, collections and cURL. — [GitHub support repo](https://github.com/thunderclient/thunder-client-support)

**HTTPie Desktop / Web**
- Latest desktop version is 2025.2.0 (2025-03-14), still labeled "public beta". Homebrew shows the same. — [httpie.io/download](https://httpie.io/download); [Homebrew cask](https://formulae.brew.sh/cask/httpie-desktop)
- Features: spaces, collections and tabs; variables and environments; preview requests without sending; "AI-assisted work"; realtime sync across devices; offline use without an account. — [httpie.io/desktop](https://httpie.io/desktop)

**RapidAPI for Mac (formerly Paw)**
- Native macOS app with JSON Schema support, import/export of Swagger/OpenAPI, RAML and API Blueprint, client code generation, and team sync with per-user branches and merges. — [paw.cloud](https://paw.cloud/)
- Status: a "PAW macOS Compatibility Notice" says macOS 27 and later "will not be tested, certified, or officially supported by Nokia". — [paw.cloud](https://paw.cloud/)
- MacGeneration reported on 16 Sep 2026 that "Nokia announces the immediate abandonment of RapidAPI for Mac (ex Paw)". This is a single-source report; the headline appears in the related-links sidebar of [MacGeneration](https://www.macg.co/news/voir/259362/paw-interface-native-et-confortable-pour-curl).
- Nokia acquired Rapid's technology assets in Nov 2024. — [Dealroom](https://app.dealroom.co/news/feed/nokia-acquires-rapidapi-for-150m-assets)
- Paw became free for individual users with 4.0 in 2022 **[pre-2025, possibly outdated]**. — [ifun.de](https://www.ifun.de/aus-paw-wird-rapidapi-fuer-privatanwender-jetzt-kostenlos-198660/)

**Apidog** (all-in-one design/debug/mock/test/doc platform; cloud-centric)
- Its official site and pricing pages returned empty on fetch, so features are unverified. Third-party price lists conflict:
  - Basic $9, Professional $18, Enterprise $27 per user/month annual; on-prem by quote — [frontdeskreview (June 2026)](https://frontdeskreview.com/software/api-testing/apidog/)
  - Basic $12, Professional $24 — [TrustRadius](https://www.trustradius.com/products/apidog/pricing)
- Apidog publishes a large volume of SEO comparison posts and syndicated "press" (e.g. "Apidog Leads the Pack"). Treat these as marketing. — [Barchart syndicated release](https://www.barchart.com/story/news/1207778/best-postman-alternatives-2026-where-to-go-after-postman-changes-pricing)

**Requestly API Client** (bundled with Requestly's HTTP interceptor)
- Protocols: REST, GraphQL (with introspection), **SOAP and gRPC** (WSDL, reflection, streaming).
- Local Workspaces/Local Projects (files) plus synced Team Workspaces. Encrypted "Vault" secrets that are never exported. Pre/post JS scripts, assertions, collection runner and **CLI for CI**. AI test-case generator. "100+ built-in data generators" (dynamic variables). Imports from Postman, Insomnia, cURL and OpenAPI.
- Integrates the **HTTP interceptor, mock server and file server**, so captured traffic can feed mocks.

[requestly.com API client](https://requestly.com/products/api-client)
- Official docs call local workspaces "early beta" with no cloud features. — [Requestly docs](https://docs.requestly.com/general/others/workspaces/local-workspace)

**Scalar API Client** (MIT; browser + desktop)
- OpenAPI-native: "Your OpenAPI document is the collection". Watches the spec file for changes. Environments. Pre-request scripts and post-response tests using a **Postman-compatible `pm` API**. Imports OpenAPI 3.x, Swagger 2.0, Postman v2.0/2.1 and cURL. Offline-first. — [scalar.com API client](https://scalar.com/products/api-client)

**JetBrains HTTP Client (in IntelliJ-family IDEs)**
- `.http`/`.rest` files with many requests per file separated by `###`.
- Protocols: HTTP/1.1, HTTP/2 (including prior knowledge), **gRPC** (unary and server-streaming), **WebSocket**, **GraphQL**, **SSE** and NDJSON streams.
- JS pre-request (`<`) and response-handler (`>`) scripts with `client.test`/`client.assert`.
- Environment files `http-client.env.json` and `http-client.private.env.json`.
- Cookie jar in `http-client.cookies`. Responses saved under `.idea/httpRequests/` with a Diff view. Last 50 requests logged in `http-requests-log.http`.
- Client certs (PEM/P12/DER) and proxy.
- cURL to and from conversion, Postman collection import, and request generation from OpenAPI or Spring/JAX-RS/Micronaut controllers.

[JetBrains docs](https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html)

**VS Code REST Client (Huachao)**
- `.http`/`.rest` files. Environments in VS Code `settings.json` (with `$shared`). `$dotenv` and `$processEnv` variables. **Request variables for chaining** via JSONPath/XPath. GraphQL. cURL import and copy-as-cURL. Code snippet generation.
- Auth: Basic, Digest, client certs (PEM/PFX), Azure AD, Microsoft Identity Platform, **AWS SigV4, AWS Cognito**. Cookies and history.
- 6.1k stars and 530 open issues.

[GitHub](https://github.com/Huachao/vscode-restclient)

**Posting (TUI)**
- Posting 3 (beta) is "a single binary written in Go", built with "Terma". Posting 2 was Python/Textual.
- YAML request files, layered environments, `.env`. HTTP, GraphQL, **gRPC (incl. streaming)**, SSE. Imports OpenAPI, Postman and Bruno; cURL import/export. Header autocomplete and "jump mode". Works over SSH.
- 12.5k stars, Apache-2.0.

[GitHub](https://github.com/darrenburns/posting)

**ATAC (TUI, Rust/ratatui)**
- reqwest + tokio, JS scripting via boa_engine (pre/post request), JSON or YAML collection files, environments.
- Auth: Basic, Bearer, Digest, JWT. **No OAuth or AWS yet.**
- **WebSocket and MQTT 3.1.1/5** clients. Cookie store.
- Imports Postman v2.1, cURL and OpenAPI. Exports to cURL, PHP Guzzle, Axios and Rust reqwest. CLI mode, vim keybindings.
- No GraphQL or gRPC yet. MIT, 3.7k stars.

[GitHub](https://github.com/Julien-cpsn/ATAC)

**Hurl (Orange; Rust CLI)**
- 8.0.0 (2026-04-24): RFC 9535 JSONPath, TLS certificate assertions and captures, `--no-cookie-store`, `--verbosity`, raw multiline strings. Renamed `--ignore-asserts` to `--no-assert` and removed `--interactive`. 8.0.1 followed on 2026-04-28.
- 7.1.0 (2025-11-18): pretty-printing, **secrets via env vars and `--secrets-file`**, per-request NTLM/Negotiate.
- 7.0.0 (2025-07-28): many filters and predicates, pinned public keys, and a fix for JS injection in HTML reports.
- 6.1.0 (2025-03-12): secrets redacted from JUnit, JSON, HTML, curl and cookie exports.
- 5.0.0 (2024-08): parallel runs and JSON report **[pre-2025]**.
- 6.0.0 (2024-12): `--curl` export **[pre-2025]**.

[GitHub releases](https://github.com/Orange-OpenSource/hurl/releases)

**New 2025–2026 entrants (low traction so far)**
- Volt ("Fast, Git-native API client written in Zig"), Show HN Mar 2026. ApiArk ("local-first Postman alternative"), Mar 2026. APIFlow Studio (Oct 2026). Apicat (Oct 2025). Each has single-digit HN points. — [HN Algolia "postman alternative"](https://hn.algolia.com/api/v1/search?query=postman%20alternative&tags=story&hitsPerPage=20)

### Inferences
- Protocol coverage synthesized from the findings above (✓ = documented; ? = not verified):
  - **Socket.IO**: Hoppscotch ✓, Insomnia ✓, Bruno ✗ (top-4 open issue), Yaak ?, Kreya ?.
  - **MQTT**: Hoppscotch ✓ and ATAC ✓ only among the verified tools.
  - **SOAP**: Requestly ✓; Kreya "coming soon" per G2.
  - **SSE**: Insomnia, Yaak, Kreya, Hoppscotch, JetBrains, Posting and Thunder Client ✓.
  - **gRPC**: Postman, Insomnia, Bruno, Yaak, Kreya, JetBrains, Posting, Thunder Client and Requestly ✓; Hoppscotch and ATAC not documented.
- Kreya has the broadest auth list (SigV4, mTLS, Kerberos, NTLM, JWT Profile) and the most unusual transports (UDS, named pipes, HTTP/3). That is the bar for a "best-in-class" auth/transport matrix.
- Mock servers became table stakes in 2025–2026: Postman 12 local mocks, Bruno 4.1 Mock Server, Insomnia (metered), Hoppscotch Business, Requestly. This weakens "request → mock" as a sole differentiator. "Request → mock → randomized data → contract test, all as files in the same repo, free" is still not offered as one free, file-based flow by any single tool.
- AI is everywhere but mostly cloud-metered (Postman credits) or enterprise-gated (Hoppscotch). Bruno 4.0's "OpenAI-compatible endpoints" likely permits local models (Ollama, LM Studio) but this was not verified. Yaak exposes MCP and agent skills rather than in-app LLM features.

### Gaps
- Not verified this session (search budget exhausted):
  - Postman's Socket.IO/MQTT support (believed present since ~2022–2023);
  - Postman's cookie/proxy/cert specifics;
  - Insomnia's full auth list (believed to include AWS IAM, NTLM, Hawk, OAuth1/2);
  - Bruno's SSE support;
  - Yaak's AWS SigV4 and client-cert support;
  - Apidog's protocol list (Apidog's pages rendered empty).
- No official per-tier checkmark matrices could be extracted from the Kreya, Thunder Client or Bruno pricing pages (checkmarks were lost in extraction).
- HTTPie Desktop: no release after 2025.2.0 found. Whether it is effectively in maintenance mode is unconfirmed.
- Paw/RapidAPI for Mac discontinuation rests on one press report (MacGeneration) plus the vendor's own macOS-27 non-support notice.

---

## Q2. How does each store data, and how diff- and merge-friendly is each format?

### Takeaway
2025–2026 saw a convergence on **one-file-per-request YAML in a repo folder**:
- Bruno v3.1 made OpenCollection YAML the default (Feb 2026);
- Postman 12 introduced split-file Collection 3.0 YAML (Mar 2026);
- Insomnia Git Sync writes YAML;
- Yaak mirrors its SQLite store to YAML.

Single-file formats (.http, Hurl) remain the most human-readable. Because many requests share one file, they are more prone to merge conflicts. App metadata churn and ordering fields (`seq`) are the main remaining diff and merge irritants.

### Cited Findings

**Bruno**
- The original `.bru` DSL is a custom plain-text markup with one request per file. Bruno says maintaining it became hard: it lacks native editor support, linting and schema tooling. YAML was chosen over TOML for nested structures. YAML parsing is 4–5x faster than BRU. YAML became the default for new collections in v3.1 (17 Feb 2026), and migration is "completely lossless". — [Bruno blog](https://blog.usebruno.com/making-yaml-the-default-in-bruno-v3.1)
- OpenCollection layout:
  - `opencollection.yml` at the root;
  - `environments/*.yml`;
  - `folder.yml` per folder;
  - one `.yml` per request with `info` (name, type, **`seq`**), `http` (method, url, body, auth), `runtime.scripts` and `settings`.
  
  [Bruno docs](https://docs.usebruno.com/opencollection-yaml/overview)
- Conflict on mixing formats: the blog says a collection must be all YAML or all BRU; the docs say collections can mix `.bru` and `.yml` during migration. — [Bruno blog](https://blog.usebruno.com/making-yaml-the-default-in-bruno-v3.1); [Bruno docs](https://docs.usebruno.com/opencollection-yaml/overview)
- Conflict on CLI support: the Bruno blog (Feb 2026) says the CLI works with YAML collections. A community `bru-converter` (v0.1.0, likely older) says the CLI "still operates on .bru files". — [Bruno blog](https://blog.usebruno.com/making-yaml-the-default-in-bruno-v3.1); [bru-converter README](https://cdn.jsdelivr.net/npm/bru-converter@0.1.0/README.md)
- OpenCollection is positioned as an open spec. An early preview was planned for 5 Jan 2026 (secondary source). — [scour.ing mirror of opencollection.com](https://scour.ing/@matmat/p/https://www.opencollection.com)
- Bruno's free Git UI covers clone, init, diffs, pull and commit. **Push, branches, remotes, stash and merge-conflict resolution are paid.** Public git providers only on the free tier; private providers need Pro. — [usebruno.com/pricing](https://www.usebruno.com/pricing)

**Postman 12 Native Git**
- Desktop only.
- Connecting a workspace creates a hidden `.postman/` config dir and a `postman/` resource dir with subfolders: collections, documents, environments, flows, globals, mocks, specs.
- Only **one folder can be connected to a workspace at a time**. The folder need not be a git repo at first.
- Flow: Local View → Git → CI push via Postman CLI → Cloud View.

[Postman Native Git setup/sync](https://learning.postman.com/latest-v-12/docs/use/native-git/setup); [overview](https://learning.postman.com/latest-v-12/docs/use/native-git/overview)
- Collection 3.0 replaces JSON blobs with YAML split into individual files. It is meant to be easier for humans to diff and for AI agents to read and write. — [heise](https://heise.de/-11198271)
- Collection v2.1 JSON (pre-v12) stored a whole collection as one JSON blob. That explains the historical diff and merge pain. — [heise](https://heise.de/-11198271)

**Insomnia**
- Four storage modes:
  - **Scratch Pad**: local, usable before login;
  - **Local Vault**: local or via Git, no cloud;
  - **Cloud Sync**: E2EE, login required;
  - **Git Sync**: third-party repo, no Insomnia Cloud dependency.
  
  [Kong docs: storage](https://developer.konghq.com/insomnia/storage/)
- Git Sync stores "project resources as YAML files". Insomnia "sometimes writes extra metadata", which can show as repo changes even when request content is unchanged. It has an in-app 3-way merge view, and supports GitHub, GitLab, Bitbucket and others via manual credentials. — [Kong docs: git sync](https://developer.konghq.com/insomnia/git-sync/)
- 2026 work targets this churn: "non-YAML conflict auto-resolution", "canonical repository output" (13.2.0-beta.0), and bidirectional sync for local projects without a remote (13.3.0). — [GitHub releases](https://github.com/Kong/insomnia/releases)
- A long-standing open request asks for the "Ability to save and work on a collection/doc in local file system" (#1638). — [Insomnia issues](https://github.com/Kong/insomnia/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)

**Yaak**
- The primary store is local **SQLite** (requests, environments, response history, settings). Optional **directory sync** writes workspace data to plain-text files for git or Dropbox. — [Yaak security & privacy docs](https://yaak.app/docs/getting-started/security-and-privacy)
- Directory sync (added 2025.1.1) writes YAML and reads YAML edits back in. It came with an integrated Git UI (branch, commit, push). — [yaak.app blog 2025.1.1](https://yaak.app/blog/2025.1.1)

**Other tools**
- **Kreya**: "git-diffable structured JSON files". — [kreya.app](https://kreya.app/)
- **JetBrains / REST Client**: many requests per `.http` file separated by `###`. JetBrains stores responses in `.idea/httpRequests/` and private env values in a separate `http-client.private.env.json`. — [JetBrains docs](https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html); [REST Client](https://github.com/Huachao/vscode-restclient)
- **Thunder Client**: Git Sync stores collections as JSON files in the project directory, but it is paid. Free users' data sits in VS Code extension storage (vendor claim). — [Apidog blog (vendor)](https://apidog.com/blog/thunder-client-pro-paywall-alternatives/); [thunderclient.com/pricing](https://www.thunderclient.com/pricing)
- **Posting**: YAML files. **ATAC**: JSON or YAML. **Requestly**: local files ("simple JSON files" per a testimonial). — [Posting](https://github.com/darrenburns/posting); [ATAC](https://github.com/Julien-cpsn/ATAC); [Requestly](https://requestly.com/products/api-client)
- **Hoppscotch**: export/import via files or GitHub gists; history syncs to cloud or local storage. No git-native file layout is documented. — [GitHub](https://github.com/hoppscotch/hoppscotch)
- **Paw**: team sync is a proprietary branch/merge system. A 2024 HN commenter said Paw files "broke when moved between machines" **[pre-2025]**. — [paw.cloud](https://paw.cloud/); [HN Bruno thread](https://news.ycombinator.com/item?id=39653718)

### Inferences
- **Diff-friendliness ranking** (best to worst):
  1. per-request YAML (Bruno OpenCollection, Postman 3.0, Yaak sync);
  2. per-request custom DSL (.bru);
  3. structured JSON per item (Kreya, Requestly, ATAC);
  4. multi-request text files (.http, Hurl): very readable, but co-edits collide;
  5. single-blob JSON exports (Postman v2.1, Insomnia v4 export);
  6. opaque DB or cloud (Hoppscotch, Paw, Postman cloud).
- **Merge-conflict hazards to design out:**
  - Integer ordering fields (Bruno's `seq`) in every request file. Reordering a folder rewrites many files and causes conflicts. Fractional or lexicographic ranks, or a per-folder order file, would localize changes.
  - App-written metadata (Insomnia admits spurious changes). Separate volatile UI state (open tabs, last-response, timestamps) from committed definitions.
  - Response history in the same tree as definitions.
- Yaak's model (DB as source of truth, files as a mirror) differs from Bruno and Postman 12 (files as source of truth). For /dev/all, "files are the source of truth; any DB is a rebuildable cache" matches the git-first promise and avoids sync-loop bugs. Yaak 2025.9.1 had to stop directory sync when the target dir is deleted (see Q3 sources).
- Postman's "one folder per workspace" and "desktop-only" Native Git limits mean a monorepo with many services needs multiple workspaces. Bruno 4.1's "multi-collection monorepo opening" is the comparable feature. Project-centric multi-collection repos are an area to do well.

### Gaps
- Exact Insomnia Git Sync YAML layout (one file per collection vs per request) is not documented on the fetched page.
- The Postman Collection 3.0 schema itself was not fetched.
- No primary source on how Yaak serializes ordering or IDs in synced YAML, or how it handles merge conflicts in synced files.

---

## Q3. How does each tool handle secrets?

### Takeaway
Every serious tool now separates "secret" values from committed definitions. Mechanisms vary:
- local encrypted stores (Bruno, Postman Local Vault, Requestly Vault);
- an OS-keychain-wrapped workspace key with committable ciphertext (Yaak);
- private env files (JetBrains);
- env-var injection (Hurl, REST Client, Posting).

**Integrations with external secret managers are almost universally paywalled at the enterprise tier** (Postman, Insomnia, Thunder Client, Bruno Ultimate; Kreya "coming soon"). Yaak's free 1Password plugin is the notable exception.

### Cited Findings
- **Bruno**
  - Secrets are stored locally in an OS-dependent location, using OS-level encryption when available and falling back to AES-256. They are stripped before committing or exporting. `.env` is supported, and secret-manager integrations exist. — [Bruno secrets docs](https://docs.usebruno.com/secrets-management/overview)
  - Secret-manager integration is Ultimate tier ($11/user/mo) per the pricing table. — [usebruno.com/pricing](https://www.usebruno.com/pricing)
  - v4.1.0 added GCP Secret Manager. v3.5.0 added secret annotations in bru files. v4.0.0 split variables and secrets tabs. — [GitHub releases](https://github.com/usebruno/bruno/releases)
- **Yaak**
  - Per-workspace encryption with **XChaCha20-Poly1305**. A master key in the **OS keychain** protects the workspace key.
  - Once enabled, auth headers, passwords and env values are encrypted by default. Encrypted values can be synced via directory sync, and recipients need the workspace key.
  - `secure(...)` template function. Response history and the DB as a whole are not encrypted.
  
  [Yaak security docs](https://yaak.app/docs/getting-started/security-and-privacy)
  - Search snippets described the cipher as "ChaCha20-Poly1305". The official doc says XChaCha20-Poly1305. — [yaak.app changelog 2025.2.0](https://yaak.app/changelog/2025.2.0)
  - The 2025.9.0 bundled plugin adds `1password.item()` via the official 1Password JS SDK. — [yaak.app blog 2025.9.0](https://yaak.app/blog/2025.9.0)
  - Environments can be marked "sharable" to include them in exports and directory sync. — [yaak.app changelog 2025.2.0](https://yaak.app/changelog/2025.2.0)
  - The OAuth2 "Hosted Redirect" callback goes through `oauth.yaak.app`; the localhost callback avoids it. — [Yaak security docs](https://yaak.app/docs/getting-started/security-and-privacy)
- **Postman**
  - **Local Vault** (all plans) is not synced to cloud and is limited to one machine. Secrets can be exported as encrypted or plaintext JSON.
  - **Shared Vault** (cloud, workspace-level) is on Solo, Team and Enterprise.
  - External integrations with **1Password, AWS Secrets Manager, Azure Key Vault and HashiCorp Vault** require **Enterprise + Advanced Security Administration add-on**.
  
  [Postman Vault docs](https://learning.postman.com/docs/sending-requests/postman-vault/postman-vault-secrets); [pricing](https://www.postman.com/pricing/)
  - Secret Scanner (local and cloud) is on all plans. — [pricing](https://www.postman.com/pricing/)
  - A 2024 HN user switched away because Postman uploaded secrets from "initial values" to its cloud. Several said enterprise security teams forbid Postman cloud credential storage **[pre-2025]**. — [HN Bruno thread](https://news.ycombinator.com/item?id=39653718)
- **Insomnia**
  - Native vault integrations (AWS, GCP, HashiCorp, Azure) are Enterprise only ($45/user/mo). — [insomnia.rest/pricing](https://insomnia.rest/pricing)
  - Git Sync "doesn't enforce secret protection"; docs recommend private environments, external vaults or repo scanners. — [Kong storage docs](https://developer.konghq.com/insomnia/storage/)
  - 13.2.0 links "vault tags persistently to local credential items". — [GitHub releases](https://github.com/Kong/insomnia/releases)
- **Thunder Client**: secrets manager integration (Azure & AWS) is Enterprise ($16). "Encrypted Environments" are listed, but the tier mapping is unclear. — [thunderclient.com/pricing](https://www.thunderclient.com/pricing)
- **Kreya**: secret-manager integration "Coming soon". — [kreya.app/pricing](https://kreya.app/pricing/)
- **Requestly**: Vault stores encrypted secrets that "are never exported". — [Requestly](https://requestly.com/products/api-client)
- **JetBrains**: `http-client.private.env.json` holds private values separate from `http-client.env.json`. — [JetBrains docs](https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html)
- **REST Client**: `$dotenv` and `$processEnv` system variables. — [GitHub](https://github.com/Huachao/vscode-restclient)
- **Hurl**: secrets via env vars and `--secrets-file` (7.1.0), redacted from reports and exports (6.1.0). — [Hurl releases](https://github.com/Orange-OpenSource/hurl/releases)
- **Posting**: reads `.env` files. — [Posting](https://github.com/darrenburns/posting)

### Inferences
- Two viable models for /dev/all:
  - **(a) Secrets never in repo.** Only references (e.g. `{{secret:stripe_key}}`) are committed. Values resolve from OS keychain, `.env.local` or 1Password/Bitwarden/Vault/AWS/GCP providers. This is the Bruno, JetBrains and Postman Local Vault approach.
  - **(b) Committable ciphertext.** This is Yaak's approach and enables team sharing via git, but adds key-distribution UX.
  
  The user's stated rule ("secrets stored only locally, never committed") maps to (a). Offering provider integrations (1Password CLI, Bitwarden, HashiCorp, AWS/GCP/Azure, `pass`, SOPS) **for free** would directly undercut the enterprise paywalls of Postman, Insomnia, Thunder Client and Bruno.
- A "secret scanner" pre-commit check (Postman has one on all plans) and redaction in history, exports, codegen and logs (as in Hurl 6.1) are cheap trust features worth matching.
- OAuth redirect relays (Yaak's `oauth.yaak.app`) are a subtle privacy leak. A strict local-first tool should default to a localhost callback.

### Gaps
- Bruno's exact secret-provider list (AWS, Azure, HashiCorp besides GCP) and its tier gating need confirmation on the secret-managers docs page (not fetched).
- Hoppscotch's secret-variable handling is not documented on the fetched README.
- Kreya's "user-specific" environment and keychain handling were not verified.

---

## Q4. Pricing tiers, backlash drivers, and what users praise and complain about

### Takeaway
The market's defining events are trust breaks:
- Postman retired Scratch Pad (Sep 2023), and in March 2026 limited the Free plan to 1 user;
- Insomnia 8 forced login and lost local data (Sep–Oct 2023), then partially reversed with Local Vault in 8.3;
- Thunder Client moved Git Sync behind a paywall (2023);
- Bruno pivoted from a one-time "Golden Edition" to subscriptions (2024–2025), which a blogger called "Bruno betrays";
- Nokia let Paw wither (2024–2026).

Users praise: no account, offline, plain files in git, speed, a perpetual or fair license. They complain about: forced cloud, data loss, bloat and RAM, paywalled basics (git push, history), and abandonment risk.

### Cited Findings

**Current pricing (late 2026)**
- **Postman**
  - New plans effective March 2026: Free (1 user; can no longer create a team, which previously allowed up to 3), Solo, Team (replaces Basic), Enterprise.
  - Legacy plans remain until renewal.
  - Collection runs and mock usage are now unlimited on all new plans.
  
  [Postman docs: about plans](https://learning.postman.com/docs/billing/about-plans)
  - Prices: Free $0, Solo $9/mo annual, Team $19/user/mo annual, Enterprise "contact sales". — [postman.com/pricing](https://www.postman.com/pricing/)
  - Third parties put Enterprise at ~$49/user/mo. — [PricingSaaS](https://pricingsaas.com/companies/postman)
- **Insomnia**: Essentials $0 (Git Sync limited to 3 users, or unlimited users with Git Sync disabled), Pro $12/user/mo annual, Enterprise $45/user/mo. A "Scratch Pad" is "local-only, free forever" and downloadable without login. — [insomnia.rest/pricing](https://insomnia.rest/pricing); [Kong storage docs](https://developer.konghq.com/insomnia/storage/)
- **Bruno**: Open Source $0 (2 workspaces, public git providers only), Pro $6/user/mo annual, Ultimate $11/user/mo annual (secret managers, SSO/SCIM, audit, AI governance, data-driven testing). — [usebruno.com/pricing](https://www.usebruno.com/pricing)
- **Yaak**
  - Free for personal use; commercial use requires a license: Individual $79/yr or $399 lifetime, Business $149/user/yr.
  - Annual licenses include a **lifetime fallback license**. 3 devices per license. 30-day trial.
  - Source is MIT; the license applies only to prebuilt binaries.
  
  [yaak.app/pricing](https://yaak.app/pricing)
- **Hoppscotch**: Free $0, Business $6/user/mo annual, Enterprise $11/user/mo annual. — [hoppscotch.com/pricing](https://hoppscotch.com/pricing)
- **Thunder Client**: Free (3 collections, 30 requests per collection, 5 imports/month), Starter $3, Business $7, Enterprise $16 (per user/mo, annual). — [thunderclient.com/pricing](https://www.thunderclient.com/pricing)
  - Third-party sources conflict on run limits: 250/month free per Apidog; 30 runs and non-commercial-only per ToolQuestor. — [Apidog (vendor)](https://apidog.com/blog/thunder-client-pro-paywall-alternatives/); [ToolQuestor](https://toolquestor.com/vs/bruno-vs-thunder-client)
- **Kreya**: Free / Pro / Enterprise (flat-rate unlimited users, offline licensing with no license server). Prices did not render. — [kreya.app/pricing](https://kreya.app/pricing/)
- **Apidog**: ~$9–12 Basic / $18–24 Professional / ~$27 Enterprise per user/mo; sources conflict. — [frontdeskreview](https://frontdeskreview.com/software/api-testing/apidog/); [TrustRadius](https://www.trustradius.com/products/apidog/pricing)
- **Free / OSS**: Scalar (MIT), Hurl, Posting (Apache-2.0), ATAC (MIT), REST Client (MIT). — [Scalar](https://scalar.com/products/api-client); [Posting](https://github.com/darrenburns/posting); [ATAC](https://github.com/Julien-cpsn/ATAC); [REST Client](https://github.com/Huachao/vscode-restclient)

**Backlash events**
- **Postman Scratch Pad**: end of life in **September 2023**, replaced by the lightweight API Client, which has no collections. Users who migrated reported missing collections and environments. One org said: "unable to come to an agreement with Postman so we are moving to Insomnia" **[pre-2025]**. — [Postman help](https://learning.postman.com/help/resolve-issues/postman-app/i-lost-my-requests-after-migrating-from-the-scratch-pad.md); [SailPoint forum](https://developer.sailpoint.com/discuss/t/postman-scratch-pad-removal/19235)
- **Postman March 2026 restructuring**: teams of 2+ now need the Team plan. The change is promoted heavily by competitors. — [Postman docs](https://learning.postman.com/docs/billing/about-plans); [Barchart/Apidog syndicated](https://www.barchart.com/story/news/1207778/best-postman-alternatives-2026-where-to-go-after-postman-changes-pricing)
- **Insomnia 8.0 (Sep 2023)**: mandatory login drew about two weeks of criticism (GitHub discussion Kong/insomnia #6590) and spawned forks such as Insomnium. 8.3 reinstated 100% local storage via Local Vault **[pre-2025]**. — [V2EX thread](https://www.v2ex.com/t/979944); [Kong blog via plushcap](https://www.plushcap.com/content/kong/blog/kong-kong-insomnia-83-brings-back-local-projects)
  - Users report "an update wiped local data and required an online account, which devastated goodwill". Insomnium was later archived. — [royathan.com, Jan 2025](https://royathan.com/blog/the-api-client-hunt/)
- **Thunder Client**: Pro tier introduced and Git Sync moved behind paywall "during 2023" (vendor claim, unsourced). — [Apidog blog (vendor)](https://apidog.com/blog/thunder-client-pro-paywall-alternatives/)
- **Bruno**
  - In 2024 users pre-ordered a one-time "Golden Edition" (~$9–19). The maintainer said Bruno is bootstrapped, rejects VC money, and plans to open-source paid features eventually **[pre-2025]**. — [HN Bruno thread (1,538 pts, 400 comments, Mar 2024)](https://news.ycombinator.com/item?id=39653718)
  - By Jan 2025 a blogger wrote "Bruno betrays": it moved to monthly subscriptions, paywalled core features such as reviewing past responses ("open core"), and offers no perpetual license. — [royathan.com](https://royathan.com/blog/the-api-client-hunt/)
- **Paw / RapidAPI for Mac**: Nokia bought Rapid assets in Nov 2024. macOS 27 is not supported, and abandonment was reported in Sep 2026. — [Dealroom](https://app.dealroom.co/news/feed/nokia-acquires-rapidapi-for-150m-assets); [paw.cloud](https://paw.cloud/); [MacGeneration](https://www.macg.co/news/voir/259362/paw-interface-native-et-confortable-pour-curl)

**What users complain about** (HN Bruno thread, Mar 2024 **[pre-2025 but representative]**)
- Forced cloud login and account requirements were the most common theme: "Forced login is added, users are angry and look for alternative" (nokeya). Data loss after updates (Postman, Insomnia, Insomnium). Enterprise security teams forbid cloud credential storage. — [HN 39653718](https://news.ycombinator.com/item?id=39653718)
- Bloat and Electron memory: "Postman has become bloated", with slow startup even on new machines. — [HN 39653718](https://news.ycombinator.com/item?id=39653718)
- Missing features in Bruno at the time: global vars and env overrides, automatic token refresh, gRPC/WebSocket/SSE, bulk header edit, path params, `.http` file support, failed connections registering as passed tests, and no exporting of variables from CLI runs. — [HN 39653718](https://news.ycombinator.com/item?id=39653718)

**What users praise and want** (2025–2026)
- Yaak Show HN (Feb 2025, 153 pts):
  - Praise for a clean, simple UI ("a joy to use"), open source, and a responsive author.
  - Concerns: "How can we be sure that Yaak won't be sold and enshittified two years from now?"; that the paid license was not clear on the landing page; maintenance risk (citing httpie.io).
  - Requests: assertions runnable in CI without SaaS, pre/post scripts, environment templates and inheritance.
  
  [HN 43185836](https://news.ycombinator.com/item?id=43185836)
- A Jan 2025 blogger's must-haves: local data with no forced account, no lock-in, auto-save, **request/response history including comparing past responses**, low memory, fair pricing with a perpetual option, and active maintenance. — [royathan.com](https://royathan.com/blog/the-api-client-hunt/)
- The same blogger on other tools: REST Client has no response-body history; JetBrains HTTP Client is "very slow startup unless the IDE is already running"; Hoppscotch is "bloated, with weak request/response history". — [royathan.com](https://royathan.com/blog/the-api-client-hunt/)
- A Tildes user called Bruno "almost exactly Postman without all the collaboration or cloud stuff". — [Tildes](https://tildes.net/~tech/1qyk/is_there_a_postman_alternative_without_the_bloat)
- HN traction (points):
  - Bruno launch: 1,538 (Mar 2024).
  - Yaak: Show HN 153 (Feb 2025); "now open source" 145 (Sep 2024); "Yaak 2.0" 82 (Feb 2025).
  - Kreya Show HN: 71 (2022).
  
  [HN Algolia: postman alternative](https://hn.algolia.com/api/v1/search?query=postman%20alternative&tags=story&hitsPerPage=20); [HN Algolia: yaak](https://hn.algolia.com/api/v1/search?query=yaak&tags=story&hitsPerPage=20)

### Inferences
- The recurring cycle: a free local tool gains users, then gets funding or acquisition, then forces login, cloud or paywalls, and users migrate. That makes **credible commitments** a product feature: an OSS core, data in open formats, a perpetual fallback license (Yaak's model), and no account requirement.
- Paywalling git *push* or branches (Bruno), git sync itself (Thunder Client), secret managers (everyone) or response history (Bruno, Kreya) is what users resent most. These are "basic local workflow" features. Paid tiers that charge for team, SSO or audit features generate little resentment.
- Abandonment risk (Paw, HTTPie Desktop stagnant since Mar 2025, Insomnium archived) is a live concern. A tool that stores everything in documented open formats (OpenCollection, .http, Hurl) lets users leave, which paradoxically builds trust.

### Gaps
- Primary Reddit threads could not be retrieved (search budget). Sentiment above leans on HN and blogs.
- Kong's official 2023 statement and the exact 8.3 release date were not retrieved.
- Thunder Client's own announcement of the paywall was not found. Current free-tier run limits conflict across sources.
- The current Bruno OSS-vs-paid feature line for response history was not verified on the pricing page (the "History" row appears in all tiers).

---

## Q5. Technology stacks and reported performance and memory implications

### Takeaway
- Electron dominates the incumbents: Postman, Bruno and, by prior knowledge, Insomnia.
- Tauri is used by the two most "lightweight-positioned" GUI clients, Yaak and Hoppscotch Desktop.
- Native or terminal options are Paw (dying), Posting (Go in v3), ATAC (Rust) and Hurl (Rust).

The available memory numbers are anecdotal or vendor-sourced. Directionally, Tauri apps use roughly half the RAM of Electron peers, and Postman is the heaviest.

### Cited Findings
- **Yaak**: "Rust and ReactJS" via Tauri. — [GitHub](https://github.com/mountain-loop/yaak); [HN 43185836](https://news.ycombinator.com/item?id=43185836)
  - One user's test: Yaak ~230 MB vs Bruno ~500 MB RAM. — [royathan.com](https://royathan.com/blog/the-api-client-hunt/)
- **Hoppscotch Desktop**: "built on top of the Hoppscotch Web Client and is powered by Tauri". Desktop version 26.6.0 equals release 2026.6.0, because a Windows MSI major version cannot exceed 255. — [Hoppscotch docs](https://docs.hoppscotch.io/documentation/clients/desktop)
- **Bruno**: described as a "local-first Electron app". One comparison page claims ~180 MB idle RAM vs 1,420 MB for Postman, and 350 ms vs 4,200 ms cold start. There is **no methodology, so treat it with caution**. — [softwaretestpilot](https://softwaretestpilot.com/compare/postman-vs-insomnia-vs-bruno)
  - Bruno v4.0 added a file-cache "faster collection mount" (beta). v3.5 added a collection-mount benchmark framework, which suggests large-collection load time was a known issue. — [Bruno releases](https://github.com/usebruno/bruno/releases)
  - YAML parsing is 4–5x faster than BRU. — [Bruno blog](https://blog.usebruno.com/making-yaml-the-default-in-bruno-v3.1)
- **Postman**: Electron. A vendor (Apidog) claims 6–9 s cold start, ~280 MB at launch on an M2, 700–900 MB after 2 hours, and over 1 GB after running a 50-request collection (vendor claim). — [Apidog blog (vendor)](https://apidog.com/blog/postman-slow-bloated-2026-alternatives/)
  - Postman's own Nov 2025 engineering talk: launch time cut from 36 s to under 12.5 s on slow devices, and 1–2 s on Macs. — [GitNation talk](https://gitnation.com/contents/this-is-how-we-made-postman-launch-twice-as-fast)
- **Insomnia**: an open issue reports that "snap version 12.0.0 crashes on ubuntu 25.10" (#9346), a Linux packaging fragility. 13.x moved gRPC proto prep "behind an IPC boundary" and improved large-response handling. — [Insomnia issues](https://github.com/Kong/insomnia/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc); [releases](https://github.com/Kong/insomnia/releases)
- **HN sentiment**: Electron complaints about memory were common; one user compared Discord at 1.2 GB with a native client at ~30 MB. A minority defended Electron for cross-platform openness **[pre-2025]**. — [HN 39653718](https://news.ycombinator.com/item?id=39653718)
- **TUI and CLI stacks**: Posting 3 is a Go single binary (Posting 2 was Python/Textual). ATAC is Rust + ratatui + reqwest + boa_engine JS. Paw is a native macOS app. — [Posting](https://github.com/darrenburns/posting); [ATAC](https://github.com/Julien-cpsn/ATAC); [paw.cloud](https://paw.cloud/)
- **Security costs of embedded JS sandboxes**: Bruno 4.2.1 (Sep 2026) was a security release restricting QuickJS sandbox module paths. Insomnia 13.3 added an opt-in QuickJS sandbox. Yaak 2026.7 fixed 29 dependency security advisories. — [Bruno releases](https://github.com/usebruno/bruno/releases); [Insomnia releases](https://github.com/Kong/insomnia/releases); [Yaak 2026.7.0](https://yaak.app/changelog/2026.7.0)

### Inferences
- Tauri + Rust gives /dev/all a credible memory and startup advantage over Postman, Bruno and Insomnia. Yaak has already proved the stack for this category, which also means Yaak is the closest direct competitor on stack and philosophy.
- Using Rust for the HTTP engine (e.g. reqwest/hyper, or libcurl bindings like Hurl's) avoids WebView networking limits (CORS, client certs, proxies). Yaak's 2026.1 HTTP-layer rewrite showing curl-style verbose output signals that users value raw wire visibility.
- Scripting is a trap and a requirement at once. Both Bruno and Insomnia converged on QuickJS sandboxes in 2026 after security issues. An embedded QuickJS or Boa (as in ATAC) in Rust, with a capability-based API, matches the state of the art.

### Gaps
- No independent, methodology-backed RAM and startup benchmark across Postman, Insomnia, Bruno, Yaak and Hoppscotch was found.
- Kreya's, Apidog's, Requestly's, HTTPie Desktop's and Scalar Desktop's stacks were not verified. Prior knowledge suggests Electron for most, but this is unconfirmed.
- Insomnia's Electron base is well known but was not re-verified this session.

---

## Q6. Table stakes vs differentiators in 2026, and unfulfilled power-user requests

### Takeaway
Table stakes in 2026:
- REST, GraphQL, WebSocket and gRPC;
- environments with inheritance;
- OAuth2;
- JS pre/post scripts and assertions;
- a collection runner plus CLI for CI;
- cURL, Postman and OpenAPI import;
- code generation;
- history;
- mock servers;
- git-friendly files;
- some AI/MCP hook.

Persistent unmet needs:
- durable, diffable response history;
- Socket.IO/MQTT/SSE in git-native tools;
- nested and per-env `.env` handling;
- free secret-manager integrations;
- multi-collection and monorepo workspaces;
- `.http` interop;
- OpenAPI round-trip (export and URL sync);
- tab and auto-save behavior;
- local-LLM AI.

### Cited Findings

**Most-requested open items**
- **Bruno** (open issues sorted by reactions): i18n (#322); plugins/extensions (#1529); vim mode/keyboard-centric UI (#2373); **Socket.IO** (#5887); **nested environments** (#292); **HAR import** (#1636); OAuth2 token reset per environment (#4392); **MCP server integration** (#4806); tabs from multiple collections (#5807); **`.env` file per environment** (#409); save-all (#737). — [Bruno issues](https://github.com/usebruno/bruno/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)
- **Insomnia** (open issues sorted by reactions): **temporary tabs that don't auto-save** (#1003); **shared fragments** (reusable snippets, #1784); "Tabs are half baked" (#8488); **jq for JSON filters** (#777); multi-select in tree (#493); **save and work on a collection in the local file system** (#1638); import a GraphQL schema as a collection (#2045); open-source sync server (#1752). — [Insomnia issues](https://github.com/Kong/insomnia/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)
- **Yaak** (feedback board):
  - Multi-tab support: 66 votes, Under Consideration.
  - Request scripting: 64, Under Consideration.
  - Response history: 47, In Progress.
  - **Export to OpenAPI**: 37, Backlog.
  - Ordering workspaces: 35.
  - **Save request/response examples**: 23.
  - **OpenAPI URL sync for import**: 22.
  - Global history: 13.
  - **Nested environment variables**: 15.
  
  [yaak.app/feedback](https://yaak.app/feedback)
- **HN asks** for Bruno (2024): automatic token refresh, global vars and env overrides, gRPC/WS/SSE, `.http` file support, bulk header edit. For Yaak (2025): CI-runnable assertions without SaaS, scripting, environment templates. — [HN 39653718](https://news.ycombinator.com/item?id=39653718); [HN 43185836](https://news.ycombinator.com/item?id=43185836)
- **Response history and comparing past responses** is a must-have that the blogger found missing or paywalled in REST Client, Bruno, Hoppscotch and curl. — [royathan.com](https://royathan.com/blog/the-api-client-hunt/)
- **Existing differentiators worth noting**:
  - Kreya: snapshot tests that diff responses against baselines; HTTP/3, UDS and named pipes; SigV4, Kerberos and NTLM. — [kreya.app](https://kreya.app/)
  - JetBrains: response Diff view and responses saved to disk. — [JetBrains docs](https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html)
  - Requestly: interceptor → mock → API client in one tool, plus 100+ data generators. — [Requestly](https://requestly.com/products/api-client)
  - Scalar: OpenAPI-as-collection with file watching, and a `pm`-compatible script API. — [Scalar](https://scalar.com/products/api-client)
  - Hurl: plain-text tests with captures and chaining, parallel runs, JUnit/TAP/HTML reports, and secrets redaction. — [Hurl releases](https://github.com/Orange-OpenSource/hurl/releases)
  - Yaak: rich template functions and a plugin system instead of scripts; a readable LLM-streaming view; MCP server and agent skill. — [Yaak GitHub](https://github.com/mountain-loop/yaak); [newreleases 2026.5.0](https://newreleases.io/project/github/mountain-loop/yaak/release/v2026.5.0)
- **AI in 2026**:
  - Postman Agent Mode (cloud credits). — [heise](https://heise.de/-11198271); [pricing](https://www.postman.com/pricing/)
  - Bruno 4.0 AI with OpenAI-compatible endpoints. — [Bruno releases](https://github.com/usebruno/bruno/releases)
  - Insomnia AI plugin and MCP client. — [Insomnia releases](https://github.com/Kong/insomnia/releases)
  - Hoppscotch AI agent features (Enterprise) and MCP (Business). — [Hoppscotch pricing](https://hoppscotch.com/pricing)
  - Thunder Client MCP Server (free). — [Thunder Client pricing](https://www.thunderclient.com/pricing)
  - Requestly AI test generator. — [Requestly](https://requestly.com/products/api-client)

### Inferences

**Table stakes for a 2026 HTTP client in /dev/all** (synthesized from the above)
- HTTP/1.1, HTTP/2 and ideally HTTP/3, with per-request version forcing.
- GraphQL with schema introspection, WebSocket, SSE (including LLM-stream rendering), gRPC with reflection and proto files.
- Environments with inheritance (workspace → collection → folder → request) and secret variables.
- Auth: Basic, Bearer, Digest, OAuth2 (all flows plus PKCE, auto-refresh), API key, AWS SigV4, mTLS/client certs (global and per-host), NTLM.
- Cookie jar with editing; proxy settings.
- JS pre/post scripts in a sandbox; assertions; collection runner; CLI with JUnit output.
- Import: cURL, Postman v2.1, Insomnia, OpenAPI/Swagger, HAR, Bruno, .http. Export: cURL and code generation.
- History; mock server.

**Differentiators where nobody is clearly best-in-class (opportunities)**
1. **Git-native, diffable response history and snapshots.** Store selected responses and examples as files, diff across runs, and promote to snapshot tests (Kreya-style, but free and file-based). This is the top unmet theme across Yaak, Bruno and the blog.
2. **Full protocol matrix in a git-native tool.** Socket.IO and MQTT exist only in Hoppscotch (not git-native) and ATAC (TUI). Bruno's Socket.IO request is top-4.
3. **Free secret-provider integrations** (OS keychain, 1Password, Bitwarden, HashiCorp, AWS/GCP/Azure, SOPS) plus per-env `.env` files. These are paywalled almost everywhere.
4. **Monorepo and project-centric workspaces.** Multiple collections, tabs across collections, and a "scratch" project that can be promoted into a repo. This addresses Postman's one-folder limit, Bruno's #5807, Insomnia's #1638 and the Postman/Insomnia scratch-pad history.
5. **OpenAPI round-trip.** Import, watch a URL or file for changes, and export back to OpenAPI. Yaak's #4 and #7 requests and Scalar's watch feature point to this.
6. **Request → mock → data randomization.** Turning a saved request and response into a mock route with faker-style generators exists piecemeal (Postman 12 local mocks, Bruno 4.1, Requestly). No free, file-based, cross-tool pipeline exists. This aligns with the /dev/all synergy thesis.
7. **Local / BYO-LLM AI.** Competitors' AI is cloud-metered or enterprise-gated. A local model for test generation, docs and failure diagnosis is an open lane. Bruno's OpenAI-compatible endpoint support is the nearest competitor.
8. **Interop with `.http` and Hurl.** Read and run `.http` (JetBrains/REST Client dialect) and export Hurl for CI. This was asked for in the HN Bruno thread and nobody unifies it.
9. **Tab and auto-save ergonomics.** Covers temporary or preview tabs, explicit save vs auto-save, multi-tab, and keyboard/vim. These are top items for Insomnia (#1003, #8488), Yaak (multi-tab) and Bruno (#2373).
10. **Merge-safe file design.** Avoid `seq` churn and app-metadata noise (Insomnia), and keep volatile state out of the repo.

### Gaps
- Could not access the Postman Ideas/Community board, Hoppscotch issue rankings or Reddit to cross-check unmet requests.
- Kreya's and Requestly's own roadmaps were not reviewed.
- Whether Bruno 4.0's "OpenAI-compatible endpoints" support local Ollama/LM Studio endpoints is unverified.

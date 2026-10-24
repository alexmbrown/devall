# API Mocking / Service Virtualization and HTTP Traffic Interception / Record-Replay (state as of Oct 2026)

Research date: 2026-10-21. Note on method: the web-search budget ran out near the end of the session. A few items (Postman mock-call quotas, Levo, Mocky.io hosted status, HN/Reddit threads about proxies) could not be checked and are listed under Gaps. Claims dated before 2025 are marked **[pre-2025, possibly outdated]**.

---

## 1. Mock servers: capabilities, formats, record/replay, chaos, protocols, CI, pricing

### Takeaway
No single tool covers everything. WireMock has the deepest request matching and templating, but its JSON is verbose and its multi-protocol support comes through extensions. Mockoon has the best local desktop UX, but its rules are shallow and it has no gRPC or GraphQL. Microcks and Specmatic lead on multi-protocol, spec-driven work. Prism is OpenAPI-only and stateless. Hosted tools (WireMock Cloud, Beeceptor, Postman) put stateful, chaos and team features behind paid tiers. One new single-binary competitor, mockd (Go), already pitches "import WireMock/Mockoon/HAR + chaos + MCP", which is close to the /dev/all idea.

### Cited Findings

**WireMock OSS (Java), current versions**
- The 4.x line is still in beta (4.0.0-beta.38 on the download page). It brings a Java 17 baseline and Jetty 12 by default. 3.x (3.13.2) is "entering maintenance mode" — [WebSearch summary of wiremock.org/download](https://wiremock.org/download); [community post on 4.0.0-beta.25](https://community.wiremock.io/t/33131972/hey-folks-happy-new-year-wiremock-4-0-0-beta-25-is-out-with-)
- Detailed matching, templating, stateful, recording and fault capabilities are covered in Section 2.

**WireMock Cloud (hosted SaaS)**
- The pricing page (Oct 2026) shows only **Free** and **Enterprise** (quote-based).
  - Free: 1,000 API calls/month, 1 user, 3 running mock APIs, 10 req/s, shared multi-tenant cluster.
  - Enterprise only: stateful mocking, chaos testing (faults, delays, errors), Git sync, versioning, AI/MCP (MCP server + Agent Skills), RBAC/SSO, export to WireMock OSS JSON, audit logs, K8s self-hosting.
  - Both tiers: REST, gRPC, GraphQL, Kafka, SOAP and fake data generation — [wiremock.io/pricing](https://www.wiremock.io/pricing)
- A third-party listing claims a "$236/month Team edition (5 users, 10K calls)". It is unverified and probably outdated, since the official site shows no Team tier. Other aggregators say "contact sales" — [SaaSHub via search](https://www.saashub.com/best-mock-api-software/c/rest-api); [DevTune](https://devtune.ai/verticals/api-mocking-service-virtualization/wiremock/pricing)
- Cloud-only features vs OSS (official comparison, dated July 2026):
  - Whole-API chaos injection (OSS faults are "per-stub only")
  - CSV/database-backed data sources
  - RBAC/collaboration
  - Versioning/audit trail
  - WireMock Runner (hosted control plane on your infrastructure)
  - WireMock CLI
  - gRPC/GraphQL are "via extension" in OSS — [docs.wiremock.io/concepts/comparison](https://docs.wiremock.io/concepts/comparison)

**Mockoon (Electron desktop + CLI, MIT, TypeScript)**
- Open-source feature set:
  - Unlimited mocks; route regex in Express syntax; multiple responses per route (rules or random)
  - Automatic CORS; TLS with a custom cert; latency at environment/route level
  - Stateful CRUD routes over JSON "data buckets"; callbacks/webhooks
  - Serverless deploy (AWS Lambda, Firebase); request/response logging
  - "Auto-mocking by recording a real API"; proxy mode for undefined routes
  - WebSocket support; templated file serving; OpenAPI import/export
  - CLI + Docker; offline with no account — [mockoon.com/features](https://mockoon.com/features/)
- Rule targets: body, query, header, cookie, route param, path, method, request number, global variable, data bucket, custom templating.
  - Operators: equals, regex, null, empty array, array includes, valid JSON schema (schema held in a data bucket).
  - Rules default to OR with an AND toggle, plus an invert toggle. First matching response wins. Random/sequential modes exist but cannot be combined with "disable rules" — [Mockoon dynamic rules docs](https://mockoon.com/docs/latest/route-responses/dynamic-rules/)
- Templating is Handlebars + Faker.js (v9.9.0) + custom helpers.
  - Helpers include data/dataRaw (buckets), base64, body/bodyRaw/queryParam/urlParam/header, oneOf/someOf/repeat, getGlobalVar, and JWT helpers.
  - Templating works in bodies, headers, file content, file paths and rule values. Docs version v9.9.0 — [Mockoon templating overview](https://mockoon.com/docs/latest/templating/overview/)
- 2025 retrospective: JSON Schema validation of incoming payloads, CLI watch mode, an OpenAPI import/export revamp, >900k downloads, ~8k GitHub stars, four releases in 2025 — [Mockoon 2025 retrospective](https://mockoon.com/blog/eight-years-2025-retrospective/)
- 2026 security fixes:
  - CVE-2025-59049 fixed in v9.2.0
  - Two vulnerabilities fixed in v9.7.0
  - Request-body DoS fixed in v9.8.0
  - Prototype pollution in multipart parsing fixed in v9.9.0
  - Moving to a monthly/bi-monthly release cycle; "Mockoon Pro self-hosted is now available in beta"; telemetry removed — [Mockoon blog index](https://mockoon.com/blog/)
- Pricing (Oct 2026):
  - Open source: free.
  - "Team" = self-hosted Mockoon Pro at €10/license/month (€120/yr). Billed annually, minimum 5 licenses. A license is one user seat **or** one running mock instance. Includes RBAC, audit trail and real-time collaboration.
  - Managed cloud is now offered "only as an Enterprise option".
  - 14-day trial — [mockoon.com/pricing](https://mockoon.com/pricing/)

**Stoplight Prism (SmartBear, Apache-2.0, Node)**
- Mocks from OpenAPI 2.0/3.0/3.1 and Postman Collections (`prism mock`). Has a validation proxy (`prism proxy <spec> <url>`) that reports differences from the spec.
- Recording/learning mode and data persistence are still on the **roadmap**, so it is stateless today. Requires Node ≥18.20.1. 5.1k stars — [github.com/stoplightio/prism](https://github.com/stoplightio/prism)
- Releases continue: v5.16.0 (17 Jul 2026) adds Scarf analytics. v5.15.x (Mar–Jun 2026) is mostly CI/dependency work. 115 open issues — [Prism releases](https://github.com/stoplightio/prism/releases)
- WireMock's own 2026 review calls Prism "stateless, doesn't model stateful flows" (vendor-authored) — [wiremock.io best-api-simulation-tools, 17 Apr 2026](https://www.wiremock.io/post/best-api-simulation-tools)

**MockServer (Java, James D. Bloom)**
- A search summary of Maven Central lists 6.0.0 (21 May 2026), with 6.1.0 and 7.x following May–July 2026. 5.15.0 was Jan 2023. **Could not verify directly** (mvnrepository returned 403) — [search summary citing mvnrepository](https://mvnrepository.com/artifact/org.mock-server/mockserver)
- Vendor review: "Java-native library and server that pairs well with JUnit and Testcontainers; no hosted control plane, GUI or cross-team governance" — [wiremock.io review, Apr 2026](https://www.wiremock.io/post/best-api-simulation-tools)

**Microcks (CNCF Incubating, Java/Quarkus)**
- Latest is 1.15.0 (site banner, 2026). Supports OpenAPI, AsyncAPI, gRPC/Protobuf, GraphQL and SOAP. Covers mocks plus provider/consumer contract tests in CI. Runs on-prem, in the cloud or on a laptop — [microcks.io](https://microcks.io/)
- Dispatchers (request-to-response routing):
  - URI_PARTS, URI_PARAMS, URI_ELEMENTS, QUERY_ARGS (GraphQL/gRPC), QUERY_MATCH (XPath, SoapUI)
  - SCRIPT: Groovy, or JS from 1.13.x. It has access to `mockRequest`, `requestContext` and a persistent `store`
  - QUERY_HEADER (since 1.11), JSON_BODY (JSON Pointer with equals/range/size/regexp/presence)
  - FALLBACK, PROXY, PROXY_FALLBACK
- Microcks computes a "dispatch criteria" fingerprint from the examples. Responses support `{{ }}` templates using request data and JSON Pointer — [Microcks dispatching docs](https://microcks.io/documentation/explanations/dispatching/)

**Hoverfly (Go, SpectoLabs)**
- Six modes: Capture, Simulate, Spy, Synthesize, Modify, Diff. Only one mode runs at a time — [Hoverfly modes docs](https://docs.hoverfly.io/en/latest/pages/keyconcepts/modes/modes.html)
- Release cadence is active. v1.12.8 added a "capture-on-miss" flag and fixed two security advisories (a Diff-mode race crash and a post-serve HTTP client timeout). v1.12.15 is dated 21 Sep. The page omits the year, but the Go 1.27.x bumps imply 2026 — [Hoverfly releases](https://github.com/SpectoLabs/hoverfly/releases)

**Imposter (Java/Kotlin, outofcoffee)**
- Plugins: OpenAPI/Swagger, REST, SOAP/WSDL, gRPC (5.x only), HBase, SFDC, WireMock-format, WebSocket (listed).
- Scripting in JavaScript/Groovy/Java. Also offers stores/data capture, templating, advanced matching, OpenAPI validation, failure and performance simulation, rate limiting, fake data, interceptors and proxying.
- Runs as CLI, Docker, JAR or AWS Lambda, or embedded in JVM/Node tests — [docs.imposter.sh](https://docs.imposter.sh/)

**MSW (Mock Service Worker, JS)**
- In the browser it intercepts via the Service Worker API. In Node it uses class extension rather than module patching. Supports REST/HTTP, GraphQL and WebSocket, and the docs also list SSE. Docs last updated Sep 30, 2026 — [mswjs.io/docs](https://mswjs.io/docs/)
- WebSocket mocking uses `ws.link()`. A blog says it arrived in v2.6.0 (unconfirmed). It follows the WHATWG WebSocket standard only — [MSW WebSocket docs](https://mswjs.io/docs/websocket/)

**json-server (typicode)**
- v1 is still **beta** ("expect breaking changes"); stable is v0.17.4. 75.7k stars.
- Supports filters (lt/gt/ne/in/contains/startsWith/endsWith, `_where`), sort, `_page`/`_per_page`, `_embed`, and dependent delete.
- v1 removed `--delay`, replaced `_limit`/`_expand`, and makes IDs always strings — [github.com/typicode/json-server](https://github.com/typicode/json-server)

**Beeceptor (hosted SaaS)**
- Free $0: 50 req/day/endpoint, 3 rules, public endpoints, ephemeral tunnel.
- Individual $10/mo: 15k req/mo, 50 rules, AI spec-to-mock.
- Team $25/mo: 100k req/mo, 250 rules, mTLS, CI/CD API, custom domains, historical traffic.
- Scale $99/mo: 1M+ req/mo, 500 rules, forward proxy, audit logs, "API contract drift detection"; $40 per extra 1M.
- Stateful and CRUD on all plans (with object limits) — [beeceptor.com/pricing](https://beeceptor.com/pricing/)

**Postman mock servers**
- Cloud mocks:
  - They match incoming requests to saved collection examples. The default algorithm **ignores body and headers**; body and header matching are opt-in.
  - Sessions via `x-mock-session` header or `pm_mock_session` cookie with isolated `pm.state`.
  - Fixed or custom latency. Private mocks need `x-api-key` — [Postman set-up mock servers](https://learning.postman.com/docs/design-apis/mock-apis/set-up-mock-servers/)
- **Local mocks (new):**
  - In Local View of the desktop app, each mock is a folder `postman/mocks/<name>/` in a connected Git repo, holding `config.yaml` (port, restart, scenarios) and one `<scenario>.js` per scenario.
  - Handlers are Node-style (`http.createServer`) with `// @endpoint` comments and sandbox objects `pm.mock`, `pm.state`, `pm.datasets` (SQL), `pm.test`.
  - Scenarios can add latency, errors, rate limits and "chaos mode". Auto-restart on change. "Agent Mode" generates mocks from prompts.
  - Always-on deployment requires Cloud View on a paid plan — [Postman local mock servers](https://learning.postman.com/docs/design-apis/mock-apis/local-mock-servers)
- Pricing: Free $0 (1 user), Solo $9/mo (annual), Team $19/user/mo (annual), Enterprise by quote. Native Git is on all plans. Insights (traffic observability) is **Enterprise only**. Mock call quotas were not found on the page — [postman.com/pricing](https://www.postman.com/pricing/)

**Apidog (hosted/desktop API platform)**
- "Smart mock" generates data from the schema by matching property type and name with wildcard/regex rules.
- "Mock expectations" are conditional responses evaluated top-down. Priority: expectations > example > smart mock.
- Faker-style `{{$...}}` expressions plus Nunjucks loops; mock scripts for complex logic — [Apidog smart mock docs](https://docs.apidog.com/en/smart-mock-618190m0); [Apidog mock priority](https://docs.apidog.com/en/mock-priority-sequence-618208m0)

**Kong Insomnia**
- Cloud mocks cannot define multiple responses for the same route.
- Self-hosted mocks (Docker + Redis) are aimed at Enterprise. Mockbin is "source visible, not open-source" — [Kong self-hosted mocks](https://developer.konghq.com/insomnia/self-hosted-mocks/); [Kong mock servers](https://developer.konghq.com/insomnia/mock-servers/)
- Insomnia 12 adds AI generation of mock servers from natural language, a URL, JSON or an OpenAPI spec — [heise.de](https://www.heise.de/en/news/Insomnia-12-Kong-enhances-API-tool-with-MCP-support-and-AI-assistance-11059094.html?view=print)

**Smocker (Go)**
- Mocks declared in YAML or JSON. Two ports: mock and config/UI. Docker image.
- Can proxy unmatched requests and record them (per a vendor/hosting description) — [github.com/smocker-dev/smocker](https://github.com/smocker-dev/smocker); [Hostinger app listing](https://www.hostinger.com/uk/applications/smocker)

**Castle Mock**
- **Archived 7 Jul 2025**: "no longer maintained". It had REST/SOAP mocking, imports from WSDL/WADL/Swagger/RAML, forward-and-record, and random/sequence response strategies. 332 stars — [github.com/castlemock/castlemock](https://github.com/castlemock/castlemock)

**Mocky (mocky.io)**
- Hosted free on Clever Cloud with "no guarantee [of] sustainability or stability". The self-hosting README still says "Work in progress"; README titled "Lockdown Edition (2020)" **[pre-2025 content]**. 2.1k stars — [github.com/julien-lafont/Mocky](https://github.com/julien-lafont/Mocky)

**mockd (Go, Apache-2.0, ~150 stars)**
- Billed as "One binary. Seven protocols. Zero dependencies": HTTP/HTTPS, gRPC, GraphQL, WebSocket, MQTT, SSE, SOAP.
- Imports OpenAPI/Swagger, Postman, HAR, **WireMock stubs**, WSDL, cURL and (per its comparison table) Mockoon.
- Chaos profiles: "flaky" 30% errors, "slow" 200–800 ms, "offline" 503. Stateful CRUD with seed tables.
- Local MCP server with 18 tools; YAML config (`mockd.yaml`); web dashboard on admin port 4290 — [github.com/getmockd/mockd](https://github.com/getmockd/mockd)

**Keploy (record/replay, eBPF)**
- Captures inbound API calls and outbound dependency calls (HTTP, gRPC, Postgres, MySQL, Mongo, Redis) via eBPF with no code changes, and writes mocks as files committed to the repo.
- Replays them in CI and handles noisy fields such as timestamps (vendor-authored list, 14 Oct 2026) — [keploy.io API mocking tools 2026](https://keploy.io/blog/community/api-mocking-tools)

**Market framing (vendor-authored)**
- "WireMock capable but heavy; Mockoon quick but shallow ('starts to feel narrow' once state/conditional logic is needed); Postman convenient but tied to its ecosystem and account/collection" — [keploy.io](https://keploy.io/blog/community/api-mocking-tools); [WireMock review](https://www.wiremock.io/post/best-api-simulation-tools)

### Inferences
- **Common baseline (table stakes):**
  - Path/method/query/header/body matching with regex and JSONPath
  - Handlebars-style templating with Faker
  - Multiple responses per route (rules, random, sequential)
  - Latency
  - Proxy for unmatched routes plus record
  - OpenAPI import
  - CLI/Docker for CI
- **Differentiators that /dev/all can take from specific tools:**
  - Microcks' fingerprint dispatchers and PROXY_FALLBACK
  - WireMock's near-miss diagnostics and date/numeric matchers
  - Mockoon's data-bucket CRUD
  - Prism's validation proxy
  - Postman's git-folder-per-mock layout
  - Keploy's "noise field" detection for replay
  - mockd's chaos profiles
- **Pricing pattern:** stateful mocking, whole-API chaos and Git sync are paywalled in WireMock Cloud (Enterprise-only), and forward proxy/drift detection is paywalled in Beeceptor (Scale). A free local tool offering all of these is a clear value proposition.
- **Git-friendliness ranking (inferred):**
  1. mockd/Smocker YAML, and Postman local mocks as folder + JS
  2. WireMock (one JSON per stub plus `__files`, git-diffable but verbose)
  3. Mockoon (one big JSON per environment per background knowledge, unverified this session; harder to merge)
  4. Hosted-only tools (Beeceptor, Apidog cloud), which are not git-native
- mockd is the closest open-source analogue to "/dev/all mock server" (single binary, multi-protocol, WireMock import, chaos, MCP). It lacks a native desktop UI and has low adoption (~150 stars). That leaves room for a Tauri app with deeper UX.

### Gaps
- Postman mock-server call quotas per plan were not on the fetched pricing text. The search budget ran out before they could be confirmed.
- MockServer 6.x/7.x release details could not be verified directly (403). The features of the new major versions are unknown.
- Mockoon's on-disk format was not verified this session. Background knowledge says one JSON file per environment, which is less merge-friendly than per-stub files. Its exact gRPC/GraphQL status in 2026 was not verified directly; the open issues #143 and #679 suggest neither is supported.
- MSW's SSE support version and @mswjs/source / @mswjs/data status were not retrieved.
- Smocker, Imposter and Microcks latest version dates were not retrieved.
- Mocky.io current uptime/status was not verified.

---

## 2. WireMock deep dive: stub format, admin API, recording, scenarios, templating, extensions, Cloud paywall, third-party GUIs

### Takeaway
WireMock is the de facto stub format. It has one JSON file per stub in `mappings/` plus bodies in `__files/`, a rich matcher set, Handlebars templating with ~60 helpers, scenarios, recording, webhooks and faults, and a full admin REST API described in OpenAPI. Existing GUIs are thin web front-ends over the admin API. None offers a git-aware, file-first desktop workflow, traffic-to-stub conversion from an external proxy, or a visual stateful-scenario editor.

### Cited Findings

**Stub mapping JSON format**
- Top-level fields: `request`, `response`, `priority`, `id`, `metadata`.
  - Priority: 1 is highest; unset stubs default to 5. Without priorities, the most recently added matching stub wins.
  - `metadata` holds arbitrary tags; stubs can be found or removed by JSONPath over it.
- Response fields: `status`, `statusMessage`, `headers` (string or array), `body`, `jsonBody`, `base64Body`, `bodyFileName` (relative to `__files`).
- Files in `mappings/` load at startup. Unmatched URLs fall through to static files in `__files` — [wiremock.org/docs/stubbing](https://wiremock.org/docs/stubbing/)

**Admin API**
- Mappings:
  - `POST /__admin/mappings` (create); `PUT`/`GET`/`DELETE /__admin/mappings/{id}`; `GET /__admin/mappings?limit&offset`; `DELETE /__admin/mappings`
  - `POST /__admin/mappings/save` (persist to the mappings dir)
  - `POST /__admin/mappings/import` with `importOptions.duplicatePolicy` and `deleteAllNotInImport`
  - `POST /__admin/mappings/reset`
  - `GET`/`DELETE /__admin/mappings/unmatched` (3.13.0+) — [wiremock.org/docs/stubbing](https://wiremock.org/docs/stubbing/)
- The admin API is described in OpenAPI 3.0 at `https://wiremock.org/js/wiremock-admin-api.json`. Swagger UI is at `/__admin/docs/` on a running instance — [Admin API reference](https://wiremock.org/docs/standalone/admin-api-reference/)
- Built-in web UI path `/__admin/webapp` (per third-party deployment template) — [Zeabur template via search](https://zeabur.com/templates/JAGP77)

**Request matching**
- URL and path:
  - `url`/`urlPattern` (path+query), `urlPath`/`urlPathPattern` (path only)
  - `urlPathTemplate` (RFC 6570, 3.0.0+) with `pathParameters`
- Method `ANY`; `clientIp` (3.13.0+).
- Value operators: equalTo, equalToIgnoreCase, contains/doesNotContain, matches/doesNotMatch, absent, numeric comparisons (equalToNumber, greaterThan…, lessThan…), and multi-value `hasExactly`/`includes`.
- Body matchers:
  - `equalToJson` (ignoreArrayOrder, ignoreExtraElements, JsonUnit placeholders)
  - `matchesJsonPath` with nested matchers
  - `matchesJsonSchema` (V4–V202012, 3.4+)
  - `equalToXml` (XMLUnit placeholders, namespaceAwareness 3.12+)
  - `matchesXPath`, `binaryEqualTo`
- `multipartPatterns` (ANY/ALL).
- Date matchers: before, after, equalToDateTime, with offsets like "now +3 days" and truncation.
- Logical `and`/`or`/`not` — [wiremock.org/docs/request-matching](https://wiremock.org/docs/request-matching/)

**Response templating (Handlebars, jknack Java port)**
- Enabling: local by default (only stubs with the `response-template` transformer), or global via `globalTemplating`. Templates apply to headers, bodies, proxy URLs and body file paths.
- Request model: `request.id`, url, path, pathSegments, path variables, query, headers, cookies, body, bodyAsBase64, multipart parts.
- Helpers:
  - JSON: jsonPath, parseJson, toJson, formatJson, jsonArrayAdd, jsonMerge, jsonRemove, jsonSort (4.0.0-beta.21)
  - XML: xPath, soapXPath, formatXml
  - Dates: now, parseDate, truncateDate, date
  - Random: randomValue, pickRandom, randomInt, randomDecimal
  - Numbers and collections: numberFormat, math, range, array, arrayAdd, arrayRemove, arrayJoin
  - Encoding and parsing: base64, urlEncode, formData, regexExtract
  - System: hostname, systemValue (restricted to `wiremock.*` by default)
  - Assignment: val, assign
- Faker comes as a separate `wiremock-faker-extension` (`random` helper, Datafaker library). Custom helpers and model providers are extension points — [wiremock.org/docs/response-templating](https://wiremock.org/docs/response-templating/)

**Stateful scenarios**
- `scenarioName`, `requiredScenarioState` and `newScenarioState`; every scenario starts in `Started`.
- Admin endpoints: `GET /__admin/scenarios`, `POST /__admin/scenarios/reset`, and `PUT /__admin/scenarios/{name}/state` (empty body resets; `{"state":"x"}` sets) — [wiremock.org/docs/stateful-behaviour](https://wiremock.org/docs/stateful-behaviour/)
- OSS has no data store beyond the scenario state machine. "CSV / database-backed data sources" are Cloud-only (official comparison, July 2026) — [docs.wiremock.io comparison](https://docs.wiremock.io/concepts/comparison)

**Record and playback**
- Recorder UI at `/__admin/recorder`. API: `POST /__admin/recordings/start` (targetBaseUrl), `/stop`, `/snapshot`.
- Spec options:
  - `filters` (request pattern, ids, allowNonProxied)
  - `captureHeaders`
  - `extractBodyCriteria` (text/binary size thresholds for externalizing bodies)
  - `persist`
  - `repeatsAsScenarios` (default true: identical requests become an ordered scenario)
  - `transformers`
  - `requestBodyPattern` (auto/equalTo/equalToJson/equalToXml)
  - `outputFormat`
- Files are named like `recordables_123-<uuid>.json` — [wiremock.org/docs/record-playback](https://wiremock.org/docs/record-playback/)
- Open feature request #1399: "On record save JSON data as `jsonBody`". Recordings currently store an escaped string body, which hurts readability and diffs — [WireMock issues sorted by reactions](https://github.com/wiremock/wiremock/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)

**Faults and latency**
- Delays: per-stub `fixedDelayMilliseconds`; `delayDistribution` lognormal (median, sigma, maxValue 3.13+) or uniform (lower, upper); global delays via `/__admin/settings`; `chunkedDribbleDelay` (numberOfChunks, totalDuration).
- Faults: EMPTY_RESPONSE, MALFORMED_RESPONSE_CHUNK, RANDOM_DATA_THEN_CLOSE, CONNECTION_RESET_BY_PEER. The last "works properly only on *nix", and on Windows it "will most likely hang" — [wiremock.org/docs/simulating-faults](https://wiremock.org/docs/simulating-faults/)

**Webhooks and callbacks**
- Core since 3.1.0 via `serveEventListeners: [{name: "webhook", ...}]`. Templating works with `originalRequest`; fixed or uniform random delay.
- Since 3.7.0, webhook calls are logged as sub-events. Extensible via `WebhookTransformer` — [wiremock.org/docs/webhooks-and-callbacks](https://wiremock.org/docs/webhooks-and-callbacks/)

**gRPC extension**
- Available from 3.2.0. It reads compiled descriptor sets (not `.proto`) from the `grpc/` subdirectory. `POST /__admin/ext/grpc/reset` reloads them, but descriptors cannot be uploaded via the admin API.
- Imports need `protoc --include_imports` — [wiremock.org/docs/grpc](https://wiremock.org/docs/grpc/); [community thread](https://community.wiremock.io/t/25977499/is-it-possible-to-upload-protobuf-descriptor-files-via-admin)
- The docs nav lists WebSockets and GraphQL pages (3.x/4.x) — [webhooks page nav](https://wiremock.org/docs/webhooks-and-callbacks/)

**WireMock Cloud AI/MCP**
- First MCP server announced 2 Apr 2025.
- 17 Aug 2026: a remote MCP server for Claude Code, Codex, Copilot and Cursor, plus "Agent Skills" (build simulations, stateful/data-driven behavior, validate stubs). The in-product AI assistant is in limited beta. A local MCP is bundled with the WireMock CLI — [wiremock.io AI-native end to end](https://www.wiremock.io/post/wiremock-ai-native-end-to-end); [WireMock MCP post](https://www.wiremock.io/post/wiremock-mcp-ai-coding-agents)

**Third-party WireMock GUIs**
- **holomekc/wiremock (wiremock-gui)** is a fork of WireMock with the GUI at `/__admin/webapp`, shipped as Docker image `holomekc/wiremock-gui` that replaces the jar. 138 stars, 9 open issues.
  - Features: folder view, mapping editor, search, a test-request tool, matched/unmatched request log with cURL copy, "copy unmatched to new mapping", record/snapshot helpers, an experimental scenario state-machine view, file upload/edit, live refresh, JWT helper.
  - Limitations: no drag-and-drop into folders (mappings unsortable); state machine "experimental"; earlier XSS (<3.0.4.0) — [github.com/holomekc/wiremock](https://github.com/holomekc/wiremock)
- **wiremock-ui (Gitee mirror)**: TypeScript UI over the admin API, requires WireMock ≥2.30.0, Apache-2.0, essentially no adoption — [gitee.com/fnngj/wiremock-ui](https://gitee.com/fnngj/wiremock-ui)
- **Imagicle WireMock.GUI**: open-sourced, built around **WireMock.Net** (.NET port), not the Java server — [Imagicle blog](https://www.imagicle.com/es/blog/imagicle-news/unified-communications-collaboration/imagicle-contribution-to-open-source-wiremock-gui/)

### Inferences
- **WireMock-manager features that no surveyed GUI combines (opportunity):**
  - Edit stub files on disk in a git repo, with live hot-push to a running instance through `/__admin/mappings/import` using `deleteAllNotInImport`
  - A visual scenario/state-machine editor (holomekc's is experimental and read-mostly)
  - Near-miss / unmatched-request diff view with one-click "create stub from this request"
  - Pretty `jsonBody` on record, fixing a known pain point (#1399)
  - Template helper autocomplete and live preview against a sample request
  - Fault/latency presets, with a Windows warning for CONNECTION_RESET_BY_PEER
  - Multi-instance management (local, Docker, Testcontainers)
  - Export/import between WireMock, Mockoon and Postman formats
- **Paywall to replicate locally:** whole-API chaos, CSV/data-backed responses, Git sync, versioning and MCP tooling are WireMock Cloud Enterprise features. A local app can offer them over OSS WireMock, or over its own engine that emits WireMock-compatible JSON.
- The admin API ships as OpenAPI 3.0. /dev/all could generate a typed Rust client from `wiremock-admin-api.json` instead of hand-writing one.

### Gaps
- The full admin endpoint list was not enumerated: near-misses (`/__admin/requests/unmatched/near-misses`), `/__admin/requests/find`, `/count`, `/__admin/files`, `/__admin/settings`, `/__admin/reset`, `/__admin/shutdown`. They exist per background knowledge but were not fetched this session. Use the OpenAPI spec.
- The YAML stub support request (#2411) is open with an unknown reaction count. It is unclear whether 4.x will add YAML.
- No reliable data on other WireMock GUIs (e.g., commercial "WireMock Studio" was discontinued years ago, per background knowledge, unverified).
- The WireMock GraphQL extension's details were not retrieved.

---

## 3. Traffic interception and debugging proxies

### Takeaway
The market splits into four groups:
- **GUI commercial proxies:** Proxyman (one-time $89 license), Charles ($50, aging), Fiddler Everywhere ($7–$37/user/mo subscription), HTTP Toolkit (open-source core, Pro subscription).
- **Scriptable OSS:** mitmproxy (MIT, Python addons, Rust core for local capture).
- **Browser-level tools:** Requestly (now under the BrowserStack copyright), Chrome DevTools overrides.
- **Security-oriented:** Burp, about $499/yr.

Rules, mocks, HAR/code export and "rules from traffic" are the paid features. Per-process capture (mitmproxy local mode), automatic Docker/Android interception (HTTP Toolkit) and MCP hooks (Proxyman, Fiddler) are the 2025–26 differentiators.

### Cited Findings

**HTTP Toolkit (desktop app; HTTP engine is the mockttp library)**
- Free "Hobbyist" ("100% Open Source"): intercept, inspect, filter, manual request/response breakpoints, send requests.
- **Pro-only**:
  - Mock responses, redirection, automatic transforms, error/timeout injection, "rules from real traffic"
  - Edit-and-resend
  - HAR and code export (20+ tools); import/export of rules and traffic
  - Built-in docs for 2,600+ APIs and custom OpenAPI specs
  - TLS passthrough, client certificates, upstream proxies
- Team adds transferable licenses; team workspaces "coming soon" — [httptoolkit.com/pricing](https://httptoolkit.com/pricing/)
- Pro price is **not shown** in the fetched text. Third parties conflict: Capterra lists "$14 per user/month"; Gappsy says "~€8/month" (Jul 2026, unverified) — [Capterra](https://www.capterra.com/p/211624/HTTP-Toolkit/); [Gappsy](https://gappsy.com/tools/http-toolkit/)
- Docker: containers launched from an intercepted terminal are automatically intercepted. "Attach to Docker" recreates and restarts an existing container with interception injected — [httptoolkit.com/docker](https://httptoolkit.com/docker/)
- Integrations: Android, Docker, Python, Ruby, Java, Electron, JS, Frida scripts. Upcoming: "full scripting support, gRPC and GraphQL integration, request diffing" — [httptoolkit.com/docs](https://httptoolkit.com/docs/)
- Engine **mockttp** (TS, Apache-2.0, 890 stars) is a mock server + MITM proxy library with built-in self-signed CA generation, Node and browser support, and match/miss debugging — [github.com/httptoolkit/mockttp](https://github.com/httptoolkit/mockttp)

**Proxyman (macOS native, also Windows/Linux)**
- Trial is free but limited to one debugging rule, with no Breakpoint/Map Local/Map Remote/Scripting.
- Standard $89 one-time (1 seat, 1 year of updates). Personal $99 (2 seats + iOS/Android app seats). Team $99/seat/yr (5+ seats). Team Subscription $12/seat/mo (cloud log sharing, 10 GB).
- Features: Breakpoint, Map Local/Remote, Scripting, Diff, Network Conditions, Protobuf, JSONPath/jq filtering, **Swagger/OpenAPI export**, **MCP support**, 1-click iOS Simulator/Android Emulator capture — [proxyman.com/pricing](https://proxyman.com/pricing)

**Charles Proxy (Java)**
- Version 5.2.2. Pricing: $50 per single license; $40 for 5+; $30 for 10+; site license $400; multi-site $700 — [charlesproxy.com/buy](https://www.charlesproxy.com/buy/)

**Fiddler Everywhere (Progress Telerik)**
- Lite: $7/mo annual ($10 monthly). Rules, breakpoints, request composer, MCP, sharing and remote device capture are **not** in Lite.
- Pro: $13/mo annual ($19 monthly).
- Enterprise: $37/mo annual (SSO, offline/air-gapped, policy install).
- Fiddler Classic is not mentioned on the purchase page — [telerik.com/purchase/fiddler](https://www.telerik.com/purchase/fiddler)

**mitmproxy (Python + Rust, MIT)**
- Tools: mitmproxy (TUI), mitmweb (web UI), mitmdump (scripting). Supports HTTP/1, HTTP/2, HTTP/3, WebSocket and TLS-wrapped protocols; Protobuf decoding; Python addons — [mitmproxy.org](https://www.mitmproxy.org/)
- Release history:
  - 12.0.0 (29 Apr 2025): new contentview system (interactive Protobuf/MsgPack, contentviews writable in Rust, tree-sitter highlighting).
  - 12.1.0 (May 2025): body-content filtering in mitmweb.
  - 12.2.0 (15 Oct 2025): Python 3.14.
  - 12.2.2 (Apr 2026): fixes an LDAP injection.
  - 12.2.3 (12 May 2026): leaf cert validity cut to 197 days; HAR export fixes.
  - 11.1.0 (Jan 2025): local capture on Linux; Python ≥3.12.
  - 11.1.2 (Feb 2025): mitmweb requires an auth token after a CVE.
  - 11.0 (Oct 2024): transparent HTTP/3 **[pre-2025]** — [mitmproxy CHANGELOG](https://github.com/mitmproxy/mitmproxy/blob/main/CHANGELOG.md)
- Modes: regular, **local capture** (per-process by name or PID; Windows/Linux/macOS), WireGuard (userspace VPN server; good for mobile), reverse, transparent (Linux/macOS), TUN (Linux), upstream, SOCKS5, DNS. Repeat `--mode` to run several listeners — [docs.mitmproxy.org modes](https://docs.mitmproxy.org/stable/concepts/modes/)

**Requestly (BrowserStack)**
- API-client tiers: Team $10/user/mo; Team Pro $15/user/mo (unlimited AI, vault integrations with HashiCorp/Azure/1Password); Enterprise add-on.
- "Local Projects with Git-based collaboration and two-way GitHub/GitLab sync". Imports from Postman, OpenAPI, HAR, cURL, WSDL, SoapUI and Bruno.
- HTTP Rules (interception) pricing is on a separate page (app.requestly.io/pricing). The site is "© 2026 BrowserStack, Inc." — [requestly.com/pricing](https://requestly.com/pricing/)

**Chrome DevTools Local Overrides**
- Network panel → "Override content" / "Override headers". Overrides are saved to a local folder and served on reload. Covers fetch/XHR, documents, scripts, images and fonts. Cache is disabled while overrides are on. Elements-panel DOM edits are not saved — [developer.chrome.com/docs/devtools/overrides](https://developer.chrome.com/docs/devtools/overrides)

**Burp Suite (PortSwigger)**
- Pro is about $499/user/yr after a price rise from $449 on 6 Jan 2026; some sources cite $475 (secondary sources conflict).
- Community (free) has the proxy, Repeater, Decoder and Comparer; Intruder is throttled; no scanner or crawler — [codeant.ai Burp pricing](https://codeant.ai/blogs/burp-suite-pricing); [cipherssecurity.com, Sep 2026](https://cipherssecurity.com/burp-suite-pricing-2026-pro-vs-dast/)

### Inferences
- **Feature baseline for a /dev/all traffic inspector:**
  - Local CA generation with install helpers for OS, Firefox, Android and iOS Simulator
  - Explicit-proxy mode plus per-process/terminal launch (env vars HTTP_PROXY, NODE_EXTRA_CA_CERTS, SSL_CERT_FILE, etc.)
  - Docker attach
  - Request/response breakpoints
  - Map Local / Map Remote / rewrite rules
  - Throttling
  - HAR import/export
  - Copy as cURL/code
  - Protobuf/gRPC decode with a `.proto` picker
  - WebSocket frame view and breakpoints
  - Filter language; tagging and notes
- **Premium features competitors charge for:** mocks, rules-from-traffic, HAR/code export (HTTP Toolkit Pro); Map Local/Breakpoints/Scripting (Proxyman paid); rules and breakpoints (Fiddler Pro). /dev/all can bundle these free and local.
- **Synergy no single tool does end-to-end:** Proxyman exports OpenAPI and HTTP Toolkit has "rules from traffic". The full chain is capture → select flows → "create WireMock/Mockoon stub" (git file) → "randomize with Faker templates" → "infer OpenAPI".
- **Rust-native local capture:** mitmproxy's per-process local capture is implemented in the `mitmproxy_rs` crate (see Section 6). A Rust app could reuse it rather than reimplementing WinDivert/NetworkExtension plumbing; licensing and API stability need checking.

### Gaps
- HTTP Toolkit's current Pro/Team prices are conflicting or unavailable.
- Requestly HTTP Rules pricing and rule limits were not retrieved (separate page).
- Fiddler Classic status and Proxyman Windows/Linux feature parity were not verified.
- iOS device interception approaches (Proxyman iOS app, HTTP Toolkit iOS request #11) were not researched in depth.
- No 2025–26 HN/Reddit threads on proxies were retrieved: the search budget was exhausted.

---

## 4. Traffic-to-spec reverse engineering

### Takeaway
Open-source traffic-to-OpenAPI is fragmented and mostly heuristic:
- **mitmproxy2swagger** is the most used (~9.7k stars) but manual and two-pass.
- **Optic**'s OSS repo was **archived Jan 2026** after the Atlassian acquisition.
- Newer entrants include **Redocly `generate-spec`** (experimental, OpenAPI 3.1, deterministic baseline plus optional AI refinement with validation), **traffic2openapi** (Go, Mar 2026) and **mitm2openapi** (Rust rewrite).
- Enterprise "API discovery" (Postman Insights/Akita, eBPF) is cloud and Enterprise-only.

A local, LLM-assisted, validated spec inference is a real gap.

### Cited Findings
- **mitmproxy2swagger** (Python, ~9.7k stars):
  - Input: mitmproxy flow files or browser HAR (auto-detected).
  - Pass 1 writes `x-path-templates` with every path prefixed `ignore:`; the user un-ignores paths and sets path params. Pass 2 generates OpenAPI 3.0.
  - `--examples`/`--headers` can leak tokens and PII. Captures can be merged and existing specs extended. Existing descriptions are not overwritten — [github.com/alufers/mitmproxy2swagger](https://github.com/alufers/mitmproxy2swagger)
- **Optic**:
  - "Archived by the owner on Jan 12, 2026", read-only. "Optic Labs is now part of Atlassian".
  - Features were: build OpenAPI from test traffic (`optic capture`), schema patches, breaking-change diff (`optic diff --base main --check`), lint. ~1.5k stars — [github.com/opticdev/optic](https://github.com/opticdev/optic)
  - The acquisition was reportedly April 2024, folding into Atlassian Compass (secondary source) **[pre-2025]** — [listicler.com via search](https://listicler.com/tools/optic)
- **Redocly CLI `generate-spec`** (experimental):
  - Input: HAR, Kong, Nginx/Apache JSON logs, NDJSON. Output: OpenAPI 3.1 only.
  - The deterministic baseline is "a hypothesis: coarse types, no descriptions/enums beyond observed".
  - Optional AI refinement is accepted only if it keeps path, method and observed status codes and passes ruleset validation — [redocly.com/docs/cli/commands/generate-spec](https://redocly.com/docs/cli/commands/generate-spec)
- **traffic2openapi** (Go, v0.2.0, Mar 2026): HAR, Playwright and proxy captures → intermediate format → OpenAPI 3.0/3.1/3.2. Detects path params, auth (Bearer/Basic/API key) and pagination patterns — [pkg.go.dev/github.com/grokify/traffic2openapi](https://pkg.go.dev/github.com/grokify/traffic2openapi)
- **mitm2openapi** (Rust, 0.8.0): rewrite of mitmproxy2swagger; flows/HAR → OpenAPI 3.0; single static binary — [docs.rs/crate/mitm2openapi](https://docs.rs/crate/mitm2openapi/latest)
- **har-to-openapi** (npm library) — [npmjs.com/package/har-to-openapi](https://npmjs.com/package/har-to-openapi)
- **re-mitm2swagger**: an MCP server wrapping mitmproxy + mitmproxy2swagger with HAR→OpenAPI and secrets detection — [glama.ai listing via search](https://glama.ai/mcp/servers/mrfjtwm3hp)
- **Postman Insights (ex-Akita)**:
  - Akita was acquired Nov 2023 **[pre-2025]**. Uses eBPF passive capture; the agent is open-sourced as the "Postman Insights Agent" — [TechTarget](https://techtarget.com/searchsoftwarequality/news/366545314/Postman-API-platform-will-use-Akita-to-tame-rogue-endpoints); [API Evangelist](https://providers.apievangelist.com/providers/akita-software/)
  - Endpoints tab lists observed endpoints and accepts "hints to the algorithm".
  - The agent samples traffic: default rate limit 1,000 calls/min, and health checks can dominate. Encrypted traffic is a common reason endpoints go missing — [Postman Insights endpoints](https://learning.postman.com/docs/insights/reference/app/endpoints-tab); [Insights troubleshooting](https://learning.postman.com/docs/insights/troubleshoot/traffic.md)
  - Insights is Enterprise-plan only — [postman.com/pricing](https://www.postman.com/pricing/)
- **Specmatic** advertises "proxy recording" of live traffic to generate specs or mocks — [specmatic.io](https://specmatic.io/)
- **Proxyman** offers Swagger/OpenAPI export from captured traffic (paid tiers) — [proxyman.com/pricing](https://proxyman.com/pricing)
- **Keploy** turns captured traffic (eBPF) into test cases and mock files, not specs — [keploy.io](https://keploy.io/blog/community/api-mocking-tools)

### Inferences
- **Workable /dev/all pipeline (inferred):**
  1. HAR or proxy flows
  2. Path templating via clustering of segments (UUIDs, numbers, slugs)
  3. JSON-schema merge across samples (required = seen in all; enums for low-cardinality strings; formats such as date-time, uuid, email)
  4. Auth scheme detection
  5. Optional local-LLM pass for names, descriptions and grouping, constrained like Redocly's (keep path, method and status codes; must pass Spectral-style lint)
  6. A diff UI to accept or reject changes against an existing spec in git (Optic's archived niche)
- PII/secret redaction must be first-class before writing examples to git. mitmproxy2swagger explicitly warns that `--examples` can leak tokens.
- Optic's archival leaves an OSS gap: no maintained "spec drift from traffic" tool (capture plus diff against the committed spec). Beeceptor (Scale) and Specmatic sell drift detection.

### Gaps
- Levo.ai and other enterprise API-discovery vendors (Treblle, Salt, Traceable) were not researched; the search budget ran out.
- No independent accuracy benchmarks were found for any traffic-to-spec tool.
- Kong Inso CLI's role (lint/test of specs) was not researched.

---

## 5. Contract testing and spec validation (and how mocks tie in)

### Takeaway
Mocks and contract tests converge on "one spec drives both stub and test":
- **Pact**: consumer tests against a Pact mock produce a pact file; the provider replays it.
- **Specmatic**: the OpenAPI/AsyncAPI spec is run as both stub and generated tests.
- **Microcks**: spec examples become mocks plus conformance tests.
- **Prism**: OpenAPI mock plus validation proxy.
- **Schemathesis**: property-based fuzzing from the spec.

Dredd is effectively dead (archived, per a secondary source).

### Cited Findings
- **Pact flow:**
  - Consumer unit tests register expected request/response interactions with a Pact mock provider. The consumer code calls the mock, and that produces a pact file.
  - Provider verification replays each request against the real provider and checks the response "contains at least the expected data".
  - Provider states set up preconditions. Message pacts abstract away Kafka/SNS. A Pact Broker integrates with CI — [docs.pact.io how Pact works](https://docs.pact.io/getting_started/how_pact_works)
- **PactFlow (SmartBear)**: Capterra lists a "Starting price US$99" (unit unspecified) with a free version. Official 2026 pricing was not confirmed — [Capterra PactFlow](https://www.capterra.com/p/275329/Pactflow/)
- **Specmatic:**
  - Specs: OpenAPI, AsyncAPI, GraphQL SDL, gRPC, WSDL, Arazzo, Avro.
  - Protocols: HTTP, Kafka, gRPC, GraphQL, SQS/SNS/EventBridge, IBM MQ, RabbitMQ, MQTT, WebSocket, Redis, JDBC.
  - Stubs take expectations at startup or dynamically over HTTP. Positive/negative tests are generated from the spec. Spec-vs-spec backward-compatibility checks. Proxy recording.
  - Studio (no-code UI) and "Genie" agent/MCP tooling; it also tests MCP servers for schema drift — [specmatic.io](https://specmatic.io/)
  - Pricing conflicts: Software Advice says OSS $0 / enterprise $50/user/mo; FitGap says from $2,500/mo — [Software Advice](https://www.softwareadvice.com/product/530270-Specmatic); [FitGap](https://us.fitgap.com/products/009649/specmatic)
- **Schemathesis** (Python, MIT, Hypothesis-based):
  - 4.0 was a core rewrite claiming "3x faster, 15x lower memory", with `schemathesis.toml` and phases (examples, fuzzing, stateful, coverage).
  - 4.16.0 is dated ~25 Apr 2026 by a third party and adds `require_security_scheme()` — [newreleases 4.0.0](https://newreleases.io/project/pypi/schemathesis/release/4.0.0); [pypi.org/project/schemathesis](https://pypi.org/project/schemathesis/); [yrkan.com 4.16](https://yrkan.com/tools-updates/schemathesis-v4-16-whats-new/)
- **Dredd** (apiaryio): reportedly archived Nov 2024 and read-only. Single secondary source, unconfirmed on GitHub **[pre-2025 event]** — [apidog.com/es blog via search](https://apidog.com/es/blog/open-source-cli-tools-api-testing/); [dredd.readthedocs.io](https://dredd.readthedocs.io/)
- **Prism** validation proxy reports deviations between real traffic and the OpenAPI document — [github.com/stoplightio/prism](https://github.com/stoplightio/prism)
- **Microcks** runs provider/consumer contract tests in CI from the same artifacts it mocks — [microcks.io](https://microcks.io/)
- WireMock's vendor review says Pact "is not a mock" (it is consumer-driven contract testing) — [wiremock.io review](https://www.wiremock.io/post/best-api-simulation-tools)
- **Mock validation features:**
  - WireMock Cloud lists "API mock validation" on Free and Enterprise — [wiremock.io/pricing](https://www.wiremock.io/pricing)
  - Mockoon validates incoming payloads against JSON Schema — [Mockoon 2025 retrospective](https://mockoon.com/blog/eight-years-2025-retrospective/)
  - Beeceptor Scale has "API contract drift detection" — [beeceptor.com/pricing](https://beeceptor.com/pricing/)

### Inferences
- **Mock/contract features for /dev/all (inferred):**
  - Validate each mock stub's response against the project's OpenAPI. Flag "mock drift" when the spec changes in git, since spec and mocks live in the same repo.
  - Validate incoming requests to the mock (Prism-style), returning 400/422 with explanation.
  - A "validation proxy" mode, so live traffic is checked against the spec in the inspector.
  - Export mocks as Pact interactions, or import pact files as stubs (pact JSON is a natural mock source).
  - Optionally shell out to Schemathesis for fuzzing a running API.
- Since Dredd is unmaintained and Optic archived, the OSS "spec ↔ implementation drift" space is thinner in 2026. Specmatic is the main active OSS-core player.

### Gaps
- Pact's specification v4 format and plugin framework (gRPC/protobuf via plugins) were not fetched this session.
- PactFlow and Specmatic official 2026 pricing were not confirmed.
- Dredd's archival was not confirmed on GitHub directly.

---

## 6. Rust crates for building a mock server and MITM proxy

### Takeaway
Rust has mature building blocks:
- **hudsucker** 0.25 for MITM (hyper 1.x, rustls, rcgen CA, WebSocket).
- **http-mitm-proxy** as an alternative MITM backend.
- **rcgen** for CA and leaf certificate generation.
- For mocking: **httpmock** 0.9 (with a standalone server, YAML mocks and record/playback), **wiremock-rs** 0.6.5 and **mockito** 1.7.x, which are test-oriented libraries.
- **mitm2openapi** (Rust) for traffic→OpenAPI; **mitmproxy_rs** as a Rust local-capture core.

No Rust crate reads or serves WireMock's JSON stub format natively. A /dev/all mock engine would implement that format itself on hyper/axum.

### Cited Findings
- **hudsucker 0.25.0** (MIT/Apache-2.0):
  - "MITM HTTP/S proxy" that modifies requests, responses and WebSocket messages via the `HttpHandler`/`WebSocketHandler` traits.
  - Features: `decoder` (default), `rcgen-ca` (default, `RcgenAuthority`), `openssl-ca`, `rustls-client` (default), `native-tls-client`, `http2`, `full`.
  - Dependencies: hyper ^1.7, tokio-rustls ^0.26, hyper-tungstenite/tokio-tungstenite ^0.30, rcgen ^0.14 — [docs.rs/hudsucker](https://docs.rs/hudsucker/latest/hudsucker/)
- **http-mitm-proxy**: library "meant as a backend for tools like Burp". Signs certs on the fly and does not implement protocol parsers. Its example builds a root CA with rcgen (CA constraints, self-signed) or loads CA cert/key from PEM — [docs.rs/http-mitm-proxy](https://docs.rs/http-mitm-proxy)
- **third-wheel 0.6.0**: TLS MITM library, self-described alpha/beta — [docs.rs/crate/third-wheel](https://docs.rs/crate/third-wheel/latest)
- **derusted 0.2.0**: forward proxy for HTTPS inspection with per-domain cert generation and a cert cache — [docs.rs/crate/derusted](https://docs.rs/crate/derusted/latest)
- **soth-mitm**: another MITM crate surfaced by search; details not retrieved — [docs.rs/soth-mitm](https://docs.rs/soth-mitm)
- **mitmproxy_rs**: Rust components of mitmproxy, including the local-capture/redirect implementations. Repo exists; details not fetched — [mitmproxy_rs via search](https://docsearch.algolia.com/mcp/docs/repo/mitmproxy/mitmproxy_rs); local capture is supported on Windows, Linux and macOS — [mitmproxy modes](https://docs.mitmproxy.org/stable/concepts/modes/)
- **httpmock 0.9.0** (MIT):
  - Standalone mode with a Docker image; YAML mock files; record & playback; forward/proxy mode; HTTPS.
  - Sync and async APIs; fault and delay simulation; custom matchers; diff on failed assertions; parallel tests — [docs.rs/httpmock](https://docs.rs/httpmock/latest/httpmock/)
- **wiremock (wiremock-rs) 0.6.5**: modeled on Java WireMock; pool of background servers; tokio and async-std. Its own comparison table rates mockito and httpmock as parallel and async-capable in current versions (self-reported) — [docs.rs/wiremock](https://docs.rs/wiremock)
- **mockito**: 1.7.2, MIT, Rust ≥1.85; sync/async, parallel tests, multiple servers (third-party profile) — [dev.co mockito profile](https://dev.co/testing/open-source/lipanski-mockito)
- **mitm2openapi 0.8.0** (Rust): HAR or mitmproxy flows → OpenAPI 3.0, single static binary — [docs.rs/crate/mitm2openapi](https://docs.rs/crate/mitm2openapi/latest)
- A rust-lang forum code review (2025–26) of a "terminal TLS MITM proxy in Rust" uses ratatui, tokio, rcgen and rustls. It shows the stack is common practice — [users.rust-lang.org](https://users.rust-lang.org/t/code-review-terminal-tls-mitm-proxy-in-rust-ratatui-tokio-rcgen-rustls/141027)
- **mockd** (Go) is evidence that a single-binary, multi-protocol mock server with WireMock import is feasible in a compiled language — [github.com/getmockd/mockd](https://github.com/getmockd/mockd)

### Inferences
- **Suggested stack (inferred):**
  - hudsucker (or http-mitm-proxy) + rcgen for the inspector
  - A custom mock engine on hyper/axum that natively loads WireMock `mappings/` + `__files/`, with matchers mirroring WireMock's operator set
  - A Handlebars engine such as the `handlebars` crate with ported helpers (jsonPath, randomValue, now, …) and a Faker crate such as `fake`
  - tonic + prost-reflect for gRPC descriptor-based mocking
  - tokio-tungstenite for WebSocket
- **CA management to plan for:**
  - mitmproxy shortened leaf validity to 197 days in 12.2.3 (May 2026), likely tracking CA/B Forum lifetime reductions. A local CA should generate short-lived leaves and cache them per host.
  - HTTP Toolkit users ask to regenerate or import CA keys (#33).
- Test-oriented crates (wiremock-rs, mockito) are useful for /dev/all's own test suite, not as the product engine. httpmock's standalone server plus YAML is the nearest reusable engine but has a different format from WireMock.

### Gaps
- Crate maintenance and recency (last-release dates for hudsucker, http-mitm-proxy, httpmock) were not confirmed.
- The `rcgen` latest version and API changes were not fetched; hudsucker depends on ^0.14.
- No Rust crate implementing WireMock JSON compatibility was found; the search budget ran out before a targeted search.
- Licensing and reusability of `mitmproxy_rs` local-capture as a library were not verified.

---

## 7. User complaints and wish lists (GitHub issues, reviews, forums)

### Takeaway
Recurring pains:
- WireMock is powerful but verbose: JSON sprawl, no YAML, escaped bodies on record, SOAP awkward, extension setup "circular".
- Mockoon users want GraphQL, gRPC, SSE, a web UI, HAR import/export, per-response probabilities and an API to define routes.
- HTTP Toolkit users want iOS automation, scripting of mock rules, gRPC/protobuf schemas, MQTT, request tagging, WebSocket breakpoints and a headless/Docker proxy.
- Postman mocks are criticized for ecosystem lock-in and weak scenario logic.

### Cited Findings
- **WireMock** top open issues, sorted by 👍 (counts not visible; numbers shown may be comment counts):
  - Jetty 12 upgrade (#2513)
  - Standalone-with-extensions docs "circular" (#904)
  - Webhook access to the response object (#2359)
  - **YAML stub mapping support (#2411)**
  - Optional duplicate-ID check (#2778)
  - Don't always unzip gzipped bodies (#547)
  - Configurable `__files` dir (#1163)
  - **TTL in stub mappings (#1871)**
  - "Make SOAP stubbing awesome" (#759)
  - urlPattern regex groups (#1038)
  - **Save recorded JSON as `jsonBody` (#1399)**
  - Async verification (#2418)
  - API to reset a specific scenario (#847), which the docs now show as supported via `PUT /__admin/scenarios/{name}/state` — [WireMock issues](https://github.com/wiremock/wiremock/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc); [stateful docs](https://wiremock.org/docs/stateful-behaviour/)
- **WireMock** reviews: "JSON stub files get large and hard to maintain", "steeper learning curve", "configuration can sprawl" (vendor-authored aggregators) — [keploy.io](https://keploy.io/blog/community/api-mocking-tools); [devtoolreviews.com](https://www.devtoolreviews.com/reviews/api-mocking-tools-for-developers)
- **WireMock** on HN: as an in-JVM library it dropped connections intermittently, fixed by running standalone. Others ran thousands of tests per commit fine **[pre-2025, 2018 thread]** — [news.ycombinator.com/item?id=17934763](https://news.ycombinator.com/item?id=17934763)
- **Mockoon** top open issues by 👍 (display order):
  - GraphQL mock (#143)
  - Web UI (#663)
  - gRPC mocking (#679)
  - Light theme (#100)
  - Per-response probability for random responses (#1016)
  - **API for dynamic route setup (#629)**
  - Use callback response in the route response (#1717)
  - Create route from proxy request should add query-param rule (#723)
  - Data contract definitions (#1426)
  - **Import HAR with auto-generated routes (#1568)**
  - Per-domain TLS config in callbacks (#1318)
  - Expose JSON-schema validation details (#1692)
  - **Export logs as HAR (#1045)**
  - Flathub (#1814)
  - HTTP/SOCKS upstream proxy (#1145)
  - Client-cert auth (#1600)
  - Redirect function (#1891)
  - JSON Schema for response bodies (#2039)
  - Smaller OpenAPI-generated mock files (#1996)
  - SSE (#990) — [Mockoon issues](https://github.com/mockoon/mockoon/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)
- **Mockoon** reviews: once mocks need "substantial state, conditional behavior, or deep integration with automated test suites, Mockoon starts to feel narrow" (vendor-authored) — [keploy.io](https://keploy.io/blog/community/api-mocking-tools)
- **HTTP Toolkit** top open issues by 👍:
  - Automatic iOS interception (15) (#11)
  - Stop intercepting a source (15) (#598)
  - **Scripting of mock rule behaviour (11) (#37)**
  - HTTP fuzzing tools (10) (#49)
  - Wildcards in host allowlist (9) (#415)
  - Copy whole request+response (8) (#127)
  - **Auto-save to HAR when a rule matches (8) (#342)**
  - **gRPC decoding (7) (#44)**
  - MQTT (6) (#254)
  - **Docker image of the proxy (6) (#122)**
  - System-wide interception (5) (#8)
  - Configurable columns (5) (#222)
  - Custom Frida scripts (5) (#671)
  - **Tag/annotate requests (5) (#241)**
  - **Interactive WebSocket breakpoints (5) (#445)**
  - **Select a .proto/.desc for gRPC decoding (4) (#559)**
  - Streaming responses (4) (#312)
  - Timing columns (3) (#90)
  - Magisk root cert (3) (#609)
  - **Regenerate/import CA keys (2) (#33)**
  - Also present: headless/cloud proxy (#288), reverse-proxy incoming traffic (#520) — [HTTP Toolkit issues](https://github.com/httptoolkit/httptoolkit/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)
- **Postman mocks**: "tied to both a Postman account and a defined collection", "pricing for advanced features can be a dealbreaker", "constraining once you need scenario logic or CI integration" (aggregated reviews) — [keploy.io](https://keploy.io/blog/community/api-mocking-tools); [wiremock.io review](https://www.wiremock.io/post/best-api-simulation-tools)
- **Insomnia** cloud mocks: "You can't define multiple responses for the same route… you need to create a different mock server" — [Kong mock servers docs](https://developer.konghq.com/insomnia/mock-servers/)
- **Enterprise SV** (Parasoft, Broadcom): "proprietary formats, specialized skills, significant licensing costs", "Git-unfriendly" (vendor-authored by a competitor) — [wiremock.io review](https://www.wiremock.io/post/best-api-simulation-tools)
- **Security** is a real concern for local mock tools: Mockoon shipped multiple CVE fixes in 2025–26 (prototype pollution, body-buffering DoS) — [Mockoon blog](https://mockoon.com/blog/). Hoverfly fixed two GHSA advisories in v1.12.8 — [Hoverfly releases](https://github.com/SpectoLabs/hoverfly/releases). mitmproxy fixed a mitmweb CVE by requiring an auth token in 11.1.2 (Feb 2025) — [mitmproxy changelog](https://github.com/mitmproxy/mitmproxy/blob/main/CHANGELOG.md)
- **Trend:** MCP/agent integration is spreading in 2025–26:
  - WireMock Cloud: remote MCP + Agent Skills — [wiremock.io](https://www.wiremock.io/post/wiremock-ai-native-end-to-end)
  - Proxyman: MCP — [proxyman.com/pricing](https://proxyman.com/pricing)
  - Fiddler: MCP gated to Pro+ — [telerik.com](https://www.telerik.com/purchase/fiddler)
  - mockd: 18-tool MCP — [github.com/getmockd/mockd](https://github.com/getmockd/mockd)
  - Postman: Agent Mode generating local mocks — [Postman local mocks](https://learning.postman.com/docs/design-apis/mock-apis/local-mock-servers)
  - Insomnia 12: AI mock generation — [heise.de](https://www.heise.de/en/news/Insomnia-12-Kong-enhances-API-tool-with-MCP-support-and-AI-assistance-11059094.html?view=print)

### Inferences

**Best-in-class local mock server + traffic inspector feature list for /dev/all (synthesized from all sections)**

*Mock engine*
- WireMock-compatible on-disk format (`mappings/*.json` + `__files/`), with optional YAML authoring (WireMock #2411 demand). Import/export for Mockoon, Postman collections, HAR, pact files and OpenAPI.
- Full WireMock matcher parity: urlPathTemplate, JSONPath/XPath/JSON-schema, date/numeric, and/or/not, multipart.
- Near-miss explanation for unmatched requests.
- Handlebars templating with WireMock helper parity, plus Faker. A live template preview against a captured request.
- Stateful mocking:
  - WireMock scenarios, plus Mockoon-style CRUD data buckets and CSV/JSON data sources (a Cloud-only feature in WireMock)
  - Per-session state (Postman's `x-mock-session` idea)
  - Stub TTL (#1871)
- Chaos:
  - Per-stub and whole-server latency distributions, faults and error-rate profiles (mockd-style "flaky/slow/offline")
  - Bandwidth throttling
  - A Windows-safe connection-reset implementation (WireMock's hangs on Windows)
- Webhooks/callbacks with templating and logging.
- Proxy-fallback (Microcks PROXY_FALLBACK) plus record. Repeats become scenarios. Bodies are saved as pretty `jsonBody` (#1399). Noise-field detection for timestamps/IDs (Keploy idea).
- OpenAPI-driven mocking (Prism-style dynamic examples) and request validation. "Mock drift" checks when the spec in git changes.
- Protocols beyond HTTP, roughly in demand order: GraphQL (Mockoon #143), gRPC via descriptor or `.proto` (Mockoon #679, WireMock extension), WebSocket, SSE (Mockoon #990), then optionally MQTT/Kafka.
- Headless CLI and Docker image from the same engine for CI. An admin REST API, ideally WireMock-admin-compatible so existing clients and Testcontainers modules work.

*Traffic inspector*
- Local CA generation (rcgen) with short-lived leaves. Install helpers for OS, browser and Android/iOS simulators. CA regenerate/import (HTTP Toolkit #33).
- Capture modes: explicit proxy, per-terminal/process env injection, Docker attach, and optionally OS-level per-process capture (mitmproxy local mode).
- Breakpoints for HTTP and WebSocket (HTTP Toolkit #445), Map Local/Remote, rewrite rules, scripting of rules (HTTP Toolkit #37).
- Protobuf/gRPC decoding with a schema picker (#559). HAR import/export (Mockoon #1045/#1568). Tagging and notes (#241). Timing columns.

*Synergy (the unique angle)*
- One-click "captured flow → stub file in git → Faker-randomized template".
- "Selected flows → inferred OpenAPI" with a local-LLM refinement that must pass validation (Redocly pattern) and PII/secret redaction.
- "Spec drift" view comparing live traffic to the committed spec (the niche Optic left when archived).
- An MCP server exposing mocks, traffic and specs to coding agents. This is now a table-stakes trend, and competitors paywall it (WireMock Cloud Enterprise, Fiddler Pro).

### Gaps
- No Reddit or HN threads from 2025–26 specifically about mocking or proxy pain points were retrieved. The search budget was exhausted, and the only HN thread found is from 2018.
- GitHub reaction counts for WireMock and Mockoon issues were not visible; ordering is by GitHub's 👍 sort only.
- No user-survey data was found on how teams actually use mock-tool GUIs vs code, or on demand for git-native mock storage.

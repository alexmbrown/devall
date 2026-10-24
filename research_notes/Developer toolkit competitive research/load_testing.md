# Load / Performance Testing Tools: Survey for an Embedded Rust/Tauri Load Tester (as of Oct 2026)

Research date: 2026-10-21. Conventions used below:
- "(year inferred)" means a GitHub releases page showed only day and month. The year comes from the page's 2026 footer or from the surrounding release order.
- "[pre-2025, possibly outdated]" marks sources or facts older than 2025.
- Third-party or aggregator sources are labelled as such. Prefer the official figures where both exist.
- The web-search budget ran out near the end of research. Some items (macOS limits, a few protocol details) could not be verified and are listed under Gaps.

---

## Q1. Tool landscape: scripting model, load profiles, protocols, data/correlation, thresholds, metrics, dashboards, distributed execution, pricing

### Takeaway
The market splits three ways:
- **Code-first engines with SaaS upsells:** k6 (JS/TS, AGPL, now v2.x), Gatling (Java/Kotlin/Scala/JS DSL, OSS plus paid Enterprise), Locust (Python, MIT), Artillery (YAML/TS, MPL-style OSS CLI plus Cloud), NBomber (.NET, commercial for business use).
- **Single-shot CLI "benchmarkers":** wrk/wrk2, hey, oha, bombardier, autocannon, rewrk, Vegeta, drill.
- **API-client-integrated "performance tabs":** Postman (VU profiles, collection reuse, cloud VUh) and Apidog (100-VU cap). Insomnia has no load testing at all.

Only a few tools model open/arrival-rate load correctly, which is the standard needed for a credible built-in tester. These are k6's arrival-rate executors, Gatling's open injection, Artillery's `arrivalRate` phases, Vegeta, wrk2, and oha with `-q` and `--latency-correction`.

### Cited Findings

**Grafana k6 (OSS)**
- Latest versions:
  - v2.3.0, marked "Latest", released 21 Sep 2026 (year inferred). It adds a `--scenario` flag to run selected scenarios, a `--once` flag, experimental async `group()`, `tlsAIAFetch`, and Prometheus remote-write static labels — [k6 releases](https://github.com/grafana/k6/releases)
  - v1.8.2 (12 Oct 2026) is a security patch on the 1.x line, so Grafana maintains 1.x and 2.x in parallel — [k6 releases](https://github.com/grafana/k6/releases)
- v2.0.0 shipped 11 May 2026. It describes itself as "completing the cleanup of deprecated APIs, old commands, and obsolete configuration options." Breaking changes — [k6 releases](https://github.com/grafana/k6/releases):
  - Go module path is now `go.k6.io/k6/v2`.
  - `k6 login/pause/resume/scale/status` were removed.
  - The `externally-controlled` executor was removed.
  - The HTTP API server no longer starts by default.
  - Cloud non-threshold aborts now exit with code 97.
- Other v2.0.0 features — [k6 v2.0.0 release](https://github.com/grafana/k6/releases/tag/v2.0.0):
  - The end-of-test summary now defaults to `compact`, with `full` available. `--no-summary` became `--summary-mode=disabled`.
  - Auto extension resolution: an import such as `k6/x/redis` auto-provisions the extension.
  - The web dashboard is built into the binary via `--out=web-dashboard`.
  - The help output points to `k6 x docs` and `k6 x explore`.
- v2.1.0 (30 Jun 2026) added an opt-in feature-flag system. Its first flag is "experimental native histograms for trend metrics" — [k6 releases](https://github.com/grafana/k6/releases)
- A third-party 2026 article says k6 2.0 also adds `k6 x agent`, a built-in MCP server, a new Assertions API and broader Playwright compatibility — [QAInsights, Jun 18 2026](https://qainsights.com/?p=18529). This is not confirmed in the official v2.0.0 notes I read, so treat it as unverified.
- Executors — [k6 docs: executors](https://grafana.com/docs/k6/latest/using-k6/scenarios/executors/):
  - `shared-iterations`, `per-vu-iterations`, `constant-vus` and `ramping-vus` are closed-model.
  - `constant-arrival-rate` and `ramping-arrival-rate` are open-model.
- Protocols built in: HTTP/1.1, HTTP/2 (auto-upgrade), WebSockets and gRPC. HTTP/3 is not listed. Other protocols such as SQL, Kafka, ZeroMQ and Redis come via xk6 extensions — [k6 docs: protocols](https://grafana.com/docs/k6/latest/using-k6/protocols/)
- The web dashboard runs with `K6_WEB_DASHBOARD=true` on `localhost:5665`. It can export a self-contained HTML report via `K6_WEB_DASHBOARD_EXPORT=html-report.html`, with a default `K6_WEB_DASHBOARD_PERIOD` of 10s — [k6 docs: web dashboard](https://grafana.com/docs/k6/latest/results-output/web-dashboard/)
- Thresholds in the test file give a non-zero exit code on failure for CI. Reporting covers a terminal summary, JSON and CSV — [ContextQA comparison, Aug 17 2026](https://contextqa.com/blog/automated-load-testing-tools/)
- License is AGPL-3.0, and "needs review for commercial embedding." One goroutine runs per VU — [QAInsights, Jun 2026](https://qainsights.com/?p=18529)

**Grafana Cloud k6 (SaaS)**
- The official calculator prices at "$0.15 per VUh" using the "Fractional (v2) pricing model" — [Grafana Cloud k6 cost calculator docs](https://grafana.com/docs/grafana-cloud/cost-management-and-billing/analyze-costs/reduce-costs/performance-testing-costs/calculator/)
- Third-party plan figures (not on an official pricing page I read) — [frontdeskreview, "Updated June 2026"](https://frontdeskreview.com/software/load-testing/grafana-k6/); [CubeAPM](https://cubeapm.com/blog/grafana-cloud-pricing-and-review/):
  - Free tier: 500 VUh/month.
  - Pro: $19/month platform fee plus $0.15/VUh.
  - Enterprise: quote-only, with a minimum of $25,000/year and rates as low as $0.05/VUh.
- 13 May 2026: added cost attribution of VUh by project labels — [Grafana what's new](https://grafana.com/whats-new/2026-05-13-cost-attribution-for-performance-testing/)
- Test comparison — [Grafana Cloud k6 docs: test comparison](https://grafana.com/docs/grafana-cloud/k6/analyze-results/test-comparison/):
  - It compares against a reference run, between two runs, or across all runs of one script, but only for the same script.
  - Older docs call the reference "Set as Baseline"; newer docs call it "Star a test run".
  - The base run draws as solid lines and the comparison as dashed lines.
  - Data retention does not apply to baseline tests.

**k6 Studio (recorder / GUI)**
- Desktop app for Mac, Windows and Linux, licensed AGPL-3.0, with about 560 stars — [k6-studio GitHub](https://github.com/grafana/k6-studio)
  - The recorder launches Chrome or Chromium through a mitmproxy-based proxy and saves a HAR.
  - The Generator converts the HAR into a k6 script.
  - "Rules" handle correlation (extract and replace) and custom JS.
  - The Validator runs 1 VU for 1 iteration.
- Releases — [k6-studio releases](https://github.com/grafana/k6-studio/releases):
  - v2.0.0 (2026-07-31) made browser recording and Grafana-Assistant-powered autocorrelation generally available. It also added an Assistant-guided wizard to run tests in Grafana Cloud k6.
  - v2.1.0 (2026-08-11) exports multi-tab recordings as a browser test.

**Locust (OSS, Python, MIT)**
- Latest release is 2.46.7 (04 Oct, year inferred 2026). Recent changes — [Locust releases](https://github.com/locustio/locust/releases):
  - 2.46.6 added `wait_time.poisson()` for Poisson arrivals and a `rest` method on `HttpUser`.
  - 2.46.3 added an active-user gauge.
  - 2.46.2 fixed percentiles skewed by `response_time=None`.
  - 2.46.1 dropped Python 3.10.
  - 2.46.0 added Python 3.15.
- Distributed mode — [Locust docs: distributed](https://docs.locust.io/en/stable/running-distributed.html):
  - `--master` and `--worker`, default port 5557, with `--expect-workers` for headless runs.
  - The GIL means one worker per core.
  - `--processes` uses `fork()` and "does not work on Windows."
  - There is "almost no limit" on users per worker; use FastHttpUser for throughput.
  - locust-swarm is no longer maintained.
- The web UI shows only live stats. Teams usually pipe metrics to Grafana via InfluxDB or Prometheus — [QAInsights, Jun 2026](https://qainsights.com/?p=18529)
- CI: headless mode with an exit-code flag. "Percentile gating needs custom code in an event hook" — [ContextQA, Aug 2026](https://contextqa.com/blog/automated-load-testing-tools/)
- Locust Cloud (aggregator data only):
  - Free: 200 VUh/month, max 100 concurrent VUs. Premium: $399/month for 5,000 VUh and 1,000 VUs — [agentdeals.dev](https://agentdeals.dev/vendor/locust); [fitgap](https://us.fitgap.com/products/010729/locust)
  - The same aggregator claims the locust.cloud homepage "indicates the cloud offering is shutting down." I could not verify this; the locust.cloud fetch returned no content.
  - QAInsights (Jun 2026) lists Locust cloud as "None. Self-managed only" — [QAInsights](https://qainsights.com/?p=18529)

**Artillery (OSS CLI + Artillery Cloud)**
- Latest release is v2.0.34 (14 Aug 2026, year inferred). It added `artillery ping` with HTTP/2 by default, full ESM, TypeScript definitions and `--aws-tags`, and uses Playwright 1.62.1 — [Artillery releases](https://github.com/artilleryio/artillery/releases)
- v2.0.31 (24 Apr 2026) — [Artillery releases](https://github.com/artilleryio/artillery/releases):
  - W3C Trace Context propagation via `config.http.distributedTracing`.
  - Fargate workers self-terminate if the CLI disappears for more than 180s.
  - Azure ACI artifacts are retained in Blob Storage.
  - OpenTelemetry SDK 2.x.
  - HTTP engine moved to Got v14.
- Test script format — [Artillery docs: test script reference](https://www.artillery.io/docs/reference/test-script):
  - YAML, TS or JS, split into `config` and `scenarios`.
  - Phases: constant `arrivalRate` (new VUs/sec, an open model), `rampTo`, `arrivalCount`, `pause` and `maxVusers`.
  - CSV `payload` with random or sequence order, plus `capture` (e.g. `json: $.id_token`).
  - `ensure` plugin for SLO checks.
  - Processor hooks in `.js`/`.mjs`/`.ts`.
  - Engines: HTTP, WebSocket, Socket.IO and Playwright.
  - Bundles files for Lambda and Fargate.
- Pricing (page shows © 2026; annual billing saves 20%) — [Artillery pricing](https://www.artillery.io/pricing):

  | Tier | Price | Reports/month | Retention | Seats | Workers per distributed test | Max duration |
  |---|---|---|---|---|---|---|
  | Free | $0 | 30 | 1 month | 2 | 5 | 30 min |
  | Team | $199/month | 1,000 | 6 months | 8 | 25 | 2 h |
  | Business | $499/month | 2,500 | 18 months | unlimited | unlimited | unlimited |

  - Enterprise add-ons (SSO, audit logs) start at $1,199/month.
  - BYOC deployment into your own AWS account is available.
  - Features listed include "performance trends", shared reports and custom charts.

**Gatling (OSS Community + Enterprise)**
- Latest release is v3.16.0 (29 Sep, year not shown; probably 2026) — [Gatling releases](https://github.com/gatling/gatling/releases)
- 3.16 features — [Gatling 3.16 release notes](https://docs.gatling.io/release-notes/gatling/whats-new/3.16/):
  - New GraphQL SDK (Java, Kotlin, Scala) with APQ and WebSocket subscriptions. GraphQL is "not supported yet" for the JS/TS SDK, which confirms that a JS/TS SDK exists.
  - Per-virtual-user mTLS for HTTP and MQTT.
  - Shared queues between VUs.
  - `postCheck`, standalone WebSocket checks and the HTTP `QUERY` method.
- Open-source Gatling runs from a single injector; multi-machine load requires Enterprise. It produces HTML reports with percentile distributions, and assertions fail the CI pipeline — [ContextQA, Aug 2026](https://contextqa.com/blog/automated-load-testing-tools/)
- Enterprise pricing (page © 2026) — [Gatling pricing](https://gatling.io/pricing):
  - Basic: €89/month billed annually (€99 monthly). Up to 60,000 VUs, 1 h of testing, 1 load generator, 2 seats.
  - Team: €356/month annually (€396 monthly). Up to 180,000 VUs, 5 h, 3 load generators, 10 seats, distributed testing.
  - Enterprise: on request.
  - SSO, dedicated IPs and private locations are add-ons.
  - Community Edition: "Minimal features, for local use only."
- Run trends cover up to 10 recent runs, with bar charts of status, percentiles and throughput. Run comparison shows up to 5 runs across 11 metrics. Both are paid-only — [Gatling docs: trends](https://docs.gatling.io/reference/stats/trends/introduction/); [Gatling run trends feature](https://gatling.io/features/run-trends-and-run-comparison)
- AI analysis launched June 2026. It labels trends as improving, degrading or stable. Its 2–5 run comparison gives a verdict of Similar, Some Discrepancies or Divergent, with a confidence rating — [Gatling updates](https://gatling.io/updates/ai-analysis-trends-comparison)

**Apache JMeter (Apache-2.0)**
- The official changes page lists 5.6.3 as current. 5.6.x requires Java 8+ (17+ recommended), and "the next major release would require Java 17 or later" — [jmeter.apache.org changes](https://jmeter.apache.org/changes.html)
- **CONFLICT:** a QAInsights post claims JMeter 6.0.0 has shipped. Its changes: Java 17 minimum, SLF4J 2.x, MongoDB removed, and Open Model Thread Group delays now relative to thread-group start — [QAInsights: What's new in JMeter 6.0.0](https://qainsights.com/whats-new-in-apache-jmeter-6-0-0/). The same author's Jun 2026 comparison still calls 5.6.3 current — [QAInsights Jun 2026](https://qainsights.com/?p=18529). Treat 6.0 as unconfirmed.
- Protocol breadth: HTTP, JDBC, JMS, LDAP, MQTT, FTP and TCP, plus 60+ plugins — [QAInsights Jun 2026](https://qainsights.com/?p=18529)
  - Downsides: `.jmx` XML is "hard to diff and review in Git", the GUI is "dated", and it uses one OS thread per VU (about 300–500 threads on a mid-range machine).
  - Upside: the HTML dashboard report is "comprehensive."
- CI gating means "parsing of the results file, which is clumsier than the others" — [ContextQA](https://contextqa.com/blog/automated-load-testing-tools/)
- Cloud options: BlazeMeter (from about $149/month) and OctoPerf — [QAInsights Jun 2026](https://qainsights.com/?p=18529)

**NBomber (.NET)**
- Latest is 6.7.0, written in C#/F#. It is protocol-agnostic via plugins and has scenarios as plain code — [NBomber docs](https://nbomber.com/docs/getting-started/overview/)
  - `Simulation.Inject(rate, interval, during)` provides an open-model injection API.
  - Other features: NBomber Studio, cluster support, real-time reporting, data feeds, and xUnit/NUnit CI integration.
- License: "FREE only for personal use", so a Business license is required for organizations. The Enterprise tier adds cluster mode. Agreement v3.0 has been in effect since 1 Sep 2025 with NBomber LLC — [NBomber license](https://nbomber.com/docs/getting-started/license). Actual prices were not found.

**Postman Performance Testing**
- Load profiles: Fixed, Ramp up, Spike and Peak — all VU-count, closed-model — [Postman docs](https://learning.postman.com/docs/tests-and-scripts/performance-testing/performance-test-configuration)
  - The local VU cap depends on hardware, and scripts reduce it. Postman suggests starting with 10–20 VUs.
  - Cloud runs need at least 10 VUs, use paid plans only and are metered in VU hours.
  - Data: CSV/JSON datafiles (local only) and Datasets (local and cloud).
  - Pass/fail: a "Pass test if" rule on avg, p90, p95 or p99 response time, error %, or RPS.
  - The CLI exits non-zero on failure. `--runner postman-cloud` starts a cloud run, and setup and teardown collections are supported.
  - Runs can be compared.
- In 2026 plans, performance test runs are unlimited; older plans had monthly caps — [Postman resource usage](https://learning.postman.com/docs/administration/resource-usage/)

**Apidog / Insomnia**
- Apidog runs performance tests against existing test scenarios, "up to 100 virtual users" — [Apidog docs](https://docs.apidog.com/how-to-run-performance-test-in-apidog-748076m0); [Apidog blog](https://apidog.com/blog/locust-alternative-api-load-testing/)
  - Ramp-up is supported.
  - Test data matches in Random or Sequential mode. If VUs exceed data rows, the excess VUs do not start.
  - A live chart shows throughput, response time and errors.
- Insomnia "does not have a built-in capability for stress testing" and points users to k6, Locust or loader. Its Collection Runner does iterations and delays with CSV/JSON data for functional checks only — [Insomnia docs](https://docs.insomnia.rest/insomnia/stress-testing); [Insomnia collection runner](https://docs.insomnia.rest/insomnia/collection-runner)

**CLI benchmarkers**
- **oha (Rust)**
  - Facts — [oha GitHub](https://github.com/hatoo/oha):
    - About 10.6k stars, built on tokio and ratatui.
    - Real-time TUI by default (`--no-tui`, `--fps` 16).
    - `-q` sets a global QPS limit, and `--latency-correction` works only with `-q`.
    - `--burst-delay` and `--burst-rate` control bursts.
    - `--http2` takes `-p` for parallel requests per connection.
    - HTTP/3 is experimental behind the `http3` feature (h3 plus rustls).
    - `--output-format` accepts text, json, csv or quiet, with a JSON schema in schema.json.
    - `--db-url` writes to SQLite.
  - Version: 1.16.0 on master, Rust 2024 edition, MSRV 1.88 — [oha Cargo.toml](https://raw.githubusercontent.com/hatoo/oha/master/Cargo.toml)
- **Goose (Rust)**
  - Version 0.18.1 (2025-08-14) and 0.18.0 (2025-02-25). Described as "inspired by Locust", with about 2.74M downloads — [crates.io goose](https://crates.io/api/v1/crates/goose)
  - Apache-2.0 with about 1.0k stars. You compile your own load-test binary. An optional live dashboard (`--features dashboard`) streams RPS, users and latency via SSE, with `--dashboard-control` — [Goose GitHub](https://github.com/tag1consulting/goose)
- **Vegeta (Go)**
  - About 25.2k stars, constant-rate model — [Vegeta GitHub](https://github.com/tsenart/vegeta):
    - `-rate` defaults to 50/1s; 0 means max speed.
    - `-max-workers` sets concurrency.
    - HTTP/2 by default, plus `-h2c`.
    - Targets come in http or json formats.
    - Reports: `text`, `json`, `hdrplot` and `hist`.
    - `plot` produces an HTML latency-over-time chart.
    - Usable as a Go library (`vegeta/v12/lib`).
    - Claims it "Avoids nasty Coordinated Omission."
  - Last commit 16 Feb 2026 — [ContextQA](https://contextqa.com/blog/automated-load-testing-tools/)
- **wrk / wrk2 (C)**
  - wrk has about 40,387 stars, but its last commit was 30 Dec 2023, so it is effectively unmaintained — [ContextQA, Aug 2026](https://contextqa.com/blog/automated-load-testing-tools/)
  - wrk2 adds constant throughput via `-R` and HdrHistogram. Calibration takes 10s, and the README calls itself "experimental/development mode" — [wrk2 GitHub](https://github.com/giltene/wrk2) [pre-2025, possibly outdated]
- **hey (Go)**
  - About 20.7k stars, an "ApacheBench (ab) replacement" — [hey GitHub](https://github.com/rakyll/hey):
    - `-n`, `-c` and `-z` duration.
    - `-q` is a rate limit per worker, not global.
    - `-h2` enables HTTP/2.
    - `-o csv` writes CSV.
- **bombardier (Go)**
  - About 6.8k stars, MIT, uses fasthttp by default — [bombardier GitHub](https://github.com/codesenberg/bombardier):
    - `--http1`/`--http2` switch to net/http, which also works around fasthttp's Host header problem.
    - `-l` prints latency at p50, p75, p90 and p99.
- **autocannon (Node.js)**
  - About 8.5k stars, "HTTP/1.1 benchmarking" with pipelining — [autocannon GitHub](https://github.com/mcollina/autocannon):
    - `-R` sets an overall rate and `-r` a per-connection rate.
    - `-w` enables worker threads.
    - Programmatic API.
    - Request sequences with `setupRequest`/`onResponse` hooks.
    - NDJSON output via `-j`.
- **rewrk (Rust)**
  - Built on hyper, MIT, about 587 stars — [rewrk GitHub](https://github.com/lnx-search/rewrk):
    - HTTP/1 and HTTP/2, with "truly concurrent HTTP/2" still on its to-do list.
    - `--pct` gives p50–p99.9.
    - No pipelining.
- **drill (Rust)**
  - Version 0.9.0, about 2.3k stars, last commit 29 Jul 2026 — [drill GitHub](https://github.com/fcsonline/drill); [ContextQA](https://contextqa.com/blog/automated-load-testing-tools/)
  - Ansible-like YAML plans with `assign` for value passing between steps, CSV data, `with_items`, concurrency, iterations and rampup.
  - Thresholds compare against a stored run.
- **Tsung (Erlang)**
  - About 2.6k stars. A "multi-protocol distributed" tool covering HTTP, WebDAV, SOAP, PostgreSQL, MySQL, LDAP, MQTT, AMQP and XMPP — [Tsung GitHub](https://github.com/processone/tsung)
  - Its build badge points to travis-ci.org, which suggests low activity. This is unverified.

**Cross-tool notes**
- Maintenance as of 17 Aug 2026 (last commit) — [ContextQA](https://contextqa.com/blog/automated-load-testing-tools/):

  | Tool | Last commit |
  |---|---|
  | k6 | 17 Aug 2026 |
  | Taurus | 17 Aug 2026 |
  | JMeter | 14 Aug 2026 |
  | Artillery | 14 Aug 2026 |
  | Locust | 10 Aug 2026 |
  | oha | 2 Aug 2026 |
  | drill | 29 Jul 2026 |
  | Gatling | 27 Jul 2026 |
  | Siege | 2 Jun 2026 |
  | Vegeta | 16 Feb 2026 |
  | ali | 19 Jan 2026 |
  | wrk | 30 Dec 2023 |

- Taurus wraps JMeter, Gatling, k6 and Locust in one YAML file and normalizes their reporting — [ContextQA](https://contextqa.com/blog/automated-load-testing-tools/)
- An OctoPerf study reports "up to 15–20% variance in response times across tools running identical load." Track baselines within a single tool — [QAInsights Jun 2026](https://qainsights.com/?p=18529)

### Inferences
- Feature parity bar for a "best-in-class" built-in tester:
  - Both closed models (constant and ramping VUs) and open models (constant and ramping arrival rate), as in k6, Artillery `arrivalRate`, NBomber `Inject` and Vegeta.
  - Spike and soak expressed as stage lists.
  - A `--once` or validator mode, as in k6 v2.3 `--once` and the k6 Studio Validator.
  - Data feeders from CSV/JSON with random, sequential and unique modes.
  - Correlation by extract-and-replace (Artillery `capture`, drill `assign`, k6 Studio rules).
  - Thresholds on pXX, error rate and RPS that drive a non-zero CLI exit.
  - A live dashboard plus a self-contained HTML export (k6 web dashboard, Gatling HTML).
  - JSON/CSV/SQLite raw export (oha).
- Postman and Apidog are the closest analogues to "/dev/all" because they reuse saved requests. Their load models, though, are closed-model VU profiles only, with low or hardware-bound VU caps (Apidog 100). Neither advertises coordinated-omission correction. That leaves room for a local-first tool with an open model and HDR-correct metrics.
- Licensing matters for embedding:
  - k6 and k6 Studio are AGPL-3.0, so embedding them in a closed app is problematic; spawning the k6 binary is the safer path.
  - Usable as libraries or reference designs: Locust (MIT), JMeter (Apache-2.0), Goose (Apache-2.0), rewrk and bombardier (MIT), hdrhistogram (MIT/Apache).
  - NBomber is commercial for any org use.
- Synthesized protocol coverage (from the findings above):

  | Tool | HTTP/2 | HTTP/3 | WebSocket | gRPC | GraphQL |
  |---|---|---|---|---|---|
  | k6 | yes | — | yes | yes | — |
  | Gatling | — | — | yes | — | yes (3.16) |
  | Artillery | — | — | yes (plus Socket.IO, Playwright) | — | — |
  | oha | yes | experimental | — | — | — |
  | Vegeta | yes | — | — | — | — |
  | hey | yes | — | — | — | — |
  | bombardier | yes | — | — | — | — |
  | rewrk | yes | — | — | — | — |

  - k6 HTTP/3 support is unconfirmed.
  - Gatling HTTP/2 and gRPC support was not verified in this session.
  - Locust can drive any protocol through Python code.
  - HTTP/3 is effectively only experimental anywhere, so it is a differentiation opportunity, but low priority.

### Gaps
- No official Grafana Cloud k6 plan page was fetched. The free 500 VUh, Pro $19 and Enterprise $25k figures are third-party.
- NBomber prices, Locust Cloud's official status and prices, and BlazeMeter pricing were not verified from primary sources.
- Not verified in this session:
  - Gatling HTTP/2 and gRPC plugin status.
  - Artillery HTTP/2 in the main HTTP engine (only `artillery ping` "HTTP/2 by default" is confirmed).
  - The k6 Assertions API and MCP server, which were claimed only by a third party.
  - JMeter 6.0 GA status (conflicting sources).
- Tsung and hey latest release dates and maintenance status were not available.
- Exact release years on GitHub pages (k6, Locust, Gatling, Artillery) are inferred.

---

## Q2. Coordinated omission (CO): what it is, which tools handle it, why it matters

### Takeaway
In a closed-loop tester, each virtual user waits for the previous response before sending the next request. When the server stalls, the tester quietly stops sending, so the requests that would have been slowest are never measured. Percentiles can then look orders of magnitude better than reality; wrk2 shows p99 understated about 200x.

A credible tool must offer an open (arrival-rate) model that measures from intended send time. Where it uses a closed model, it should apply correction such as HdrHistogram `record_correct` or Goose's synthetic backfill and show raw and corrected numbers side by side.

### Cited Findings
- Gil Tene's "How NOT to Measure Latency" (QCon SF 2012, QCon London 2013, SpringOne2GX 2013) says back pressure and coordinated omission can skew results "by orders of magnitude" — [QCon London 2013 abstract](https://qconlondon.com/london-2013/qconlondon.com/london-2013/presentation/How%20NOT%20to%20Measure%20Latency.html); [slides PDF](https://qconsf.com/sf2012/dl/qcon-sanfran-2012/slides/GilTene_HowNotToMeasureLatency.pdf); [SpringOne2GX replay](https://spring.io/blog/2014/06/03/springone2gx-2013-replay-how-not-to-measure-latency) [pre-2025 but foundational; still valid]
- A short explanation: if a 200 ms stall occurs, the generator should also have produced samples at about 198, 196, 194 ms and so on. Those were never recorded because the generator was blocked — [Matt Warren blog comment](https://mattwarren.org/2014/06/23/measuring-the-impact-of-the-net-garbage-collector-an-update/) [pre-2025]
- k6 defines the problem through open and closed models — [k6 docs: open vs closed](https://grafana.com/docs/k6/latest/using-k6/scenarios/concepts/open-vs-closed/):
  - Closed model: "the start or arrival rate of new VU iterations is tightly coupled with the iteration duration."
  - Open model: "The response times of the target system no longer influence the load."
  - "In some testing literature, this problem is known as coordinated omission."
  - The arrival-rate executors use `preAllocatedVUs`. In the example, a 1 it/s rate for 1 minute completed 60 iterations using 11 VUs.
- wrk2 measures latency from when a request *should* have been sent and records into HdrHistogram — [wrk2 README](https://github.com/giltene/wrk2) [pre-2025]:
  - `--u_latency` prints an uncorrected histogram for comparison.
  - Quiet run: p99 is 10.52 ms corrected vs 5.43 ms uncorrected (about 2x).
  - With a 1.4 s stall: p99 is 1.27 s corrected vs 6.04 ms uncorrected (about 200x).
  - Timer accuracy is about ±1 ms.
- oha's `--latency-correction` corrects CO "only when `-q` is set," meaning only in rate-limited mode — [oha GitHub](https://github.com/hatoo/oha)
- Vegeta says it "Avoids nasty Coordinated Omission" because it uses a constant-rate attack model — [Vegeta GitHub](https://github.com/tsenart/vegeta)
- Goose `--co-mitigation` — [Goose book: CO mitigation](https://book.goose.rs/coordinated-omission/mitigation.html); [Goose book: CO overview](https://book.goose.rs/coordinated-omission/overview.html):
  - Modes:
    - `disabled` reports raw values only.
    - `average` is the default and uses average response time as the baseline.
    - `minimum` is for strict SLAs.
  - When a scenario loop takes more than 2x the average, Goose backfills synthetic data points. They start at the long response time and step down by the normal cadence.
  - Reports separate Raw and CO-adjusted metrics. Backfilled log rows carry a non-zero `coordinated_omission_elapsed`.
  - Its example shows 4 real plus 29 synthetic requests, so 87.9% synthetic, flagged as a "critical" CO event.
- The Rust `hdrhistogram` crate (7.6.0) provides `record_correct`, "the self-correcting record method for data sources subject to Coordinated Omission," where you supply the expected interval — [docs.rs hdrhistogram](https://docs.rs/hdrhistogram/latest/hdrhistogram/)
- Locust 2.46.6 added `wait_time.poisson()` for Poisson-distributed task arrivals, but Locust remains a user/closed-loop model — [Locust releases](https://github.com/locustio/locust/releases)
- Artillery phases define `arrivalRate` as "new VUs per second," which is an open arrival of VUs — [Artillery test script reference](https://www.artillery.io/docs/reference/test-script)

### Inferences
- Design recommendations for "/dev/all":
  - Make the open model (target RPS or arrival rate) the default for "load test this request."
  - Timestamp each request at its *scheduled* send time and record `actual_end - scheduled_start` into HDR.
  - Report "dropped/late iterations" when the VU pool can't keep up, as k6 does with `preAllocatedVUs`/`maxVUs`.
  - For closed-model (VU) runs, show raw and CO-corrected percentiles side by side, as Goose does with Raw vs CO-adjusted and wrk2 with `--u_latency`. This trains users and builds credibility.
- oha's design (correction only under `-q`) shows that correction needs a known intended rate. For pure "max throughput" runs, correction is undefined, and the UI should say so.
- Goose's backfill is heuristic, since it is based on average or minimum. Measuring against a scheduled-time open model is more principled; Goose's approach is mainly useful for closed-model user journeys.

### Gaps
- I did not find a 2025–2026 study that systematically audits which tools correct CO. Classifications above come from each tool's own docs.
- k6's `dropped_iterations` metric was not covered in the page fetched, so its exact semantics need confirming from k6 metrics docs.

---

## Q3. Rust crates/approaches for an embedded high-performance load generator; single-machine OS limits

### Takeaway
The proven Rust recipe, taken from oha's manifest, is `tokio` + `hyper` 1.x/`hyper-util` (+`h3`/`quinn` for HTTP/3) + `rustls`. Add a fast MPMC channel (`kanal`), `hickory-resolver` for DNS, `rusqlite` for per-request storage, and jemalloc on non-MSVC targets. Add the `hdrhistogram` crate (7.6.0) for mergeable, CO-correctable percentiles.

Neither oha nor any CLI exposes a library API. Goose is a library, but it is aimed at compiled user programs, with Gaggle (distributed mode) disabled since 0.17. The practical route is to build a purpose-made engine following oha's architecture.

On one desktop machine, the binding limits are:
- Ephemeral ports: Windows 49152–65535 = 16,384 ports with a 4-minute TIME_WAIT; Linux 32768–60999.
- File descriptors.
- Under-sized connection reuse.

### Cited Findings
- oha 1.16.0 dependencies — [oha Cargo.toml](https://raw.githubusercontent.com/hatoo/oha/master/Cargo.toml):
  - Core HTTP and runtime: `tokio` 1.38 (full), `hyper` 1.4 (client, http1, http2), `hyper-util` 0.1.6, `http-body-util`.
  - HTTP/3 and TLS: `h3` 0.0.8, `h3-quinn`, `quinn` 0.11.7 (optional `http3`), `rustls` 0.23.18 (default) or `native-tls`.
  - Plumbing: `kanal` 0.1.1 (channels), `hickory-resolver` 0.26, `rusqlite` 0.40.1 (bundled), `ratatui` 0.30.
  - Platform-specific: `tikv-jemallocator` 0.7 on non-MSVC targets and `rlimit` 0.11 on Unix.
  - It uses neither `reqwest` nor `hdrhistogram`; percentiles use `average` and `float-ord`.
  - It has no `[lib]` section, so it is binary-only.
  - Its dev-deps (axum, actix-web, http-mitm-proxy) show its test harness approach.
- oha calls itself "inspired by rakyll/hey". It runs c × p workers for HTTP/2, and `--no-tui` without `-q` takes "a faster code path" — [oha GitHub](https://github.com/hatoo/oha)
- `hdrhistogram` crate 7.6.0 (MIT/Apache-2.0) — [docs.rs hdrhistogram](https://docs.rs/hdrhistogram/latest/hdrhistogram/):
  - `Histogram::new` auto-resizes; `new_with_max` disables resizing, so it never reallocates.
  - `record_correct` corrects for CO, and `value_at_quantile` reads percentiles.
  - `add`/`subtract` merge histograms; `SyncHistogram` supports multi-threaded recording.
  - A serialization module is included, with `flate2` optional.
  - Memory is fixed by range and precision, and recording does not allocate.
  - Not implemented: DoubleHistogram, Recorder, textual CSV/JSON output.
- Goose as a library — [Goose GitHub](https://github.com/tag1consulting/goose); [crates.io](https://crates.io/api/v1/crates/goose); [Goose book: Gaggle](https://book.goose.rs/gaggle/overview.html):
  - You "write a Rust application that uses the Goose library" and compile it.
  - Latest release is 0.18.1 (Aug 2025).
  - Gaggle (Manager/Worker distributed mode) was "temporarily removed in Goose 0.17.0," and the docs recommend 0.16.4 for it.
  - The Goose Eggs crate provides validation helpers.
- Vegeta offers a clean library API (targeter → attacker → results → `Metrics`), but it is Go and cannot be embedded in Rust directly — [Vegeta GitHub](https://github.com/tsenart/vegeta)
- rewrk is a hyper-based Rust benchmarker that runs on Windows, macOS and Linux, with pipelining disabled for realism — [rewrk GitHub](https://github.com/lnx-search/rewrk)
- Windows limits (doc updated Feb 2026) — [Microsoft Learn: TCP/IP port exhaustion](https://learn.microsoft.com/en-us/troubleshoot/windows-client/networking/tcp-ip-port-exhaustion-troubleshooting):
  - Default dynamic port range is 49152–65535.
  - Ports in TIME_WAIT are released after "4 minutes (default)."
  - Change the range with `netsh int ipv4 set dynamicport tcp start=... num=...` (minimum range 255, minimum start 1025).
  - Symptoms: Event IDs 4227 and 4231, and `STATUS_TOO_MANY_ADDRESSES`. Exhaustion breaks *all* outbound connections on the machine, including domain sign-in and RDP.
- Linux limits — [Linux kernel ip-sysctl docs](https://www.kernel.org/doc/html/latest/networking/ip-sysctl.html):
  - `ip_local_port_range` defaults to 32768–60999.
  - `tcp_tw_reuse` defaults to 2, meaning TIME-WAIT reuse applies to loopback only. The docs say it "should not be changed without advice/request of technical experts."
  - `tcp_fin_timeout` defaults to 60s (FIN_WAIT_2).
- Locust guidance: one worker per core because of the GIL; `--processes` uses `fork()` and doesn't work on Windows — [Locust docs](https://docs.locust.io/en/stable/running-distributed.html). This is a reminder that cross-platform multi-process designs need care on Windows.

### Inferences
- **Architecture sketch**:
  - A multi-threaded tokio runtime on a dedicated thread pool, separate from Tauri's UI and runtime threads, so the UI cannot perturb timing.
  - A scheduler task emits "tickets" carrying an intended start time (open model) into a `kanal` or tokio MPMC channel. Pooled hyper connections pick them up.
  - Each worker keeps a thread-local `hdrhistogram::Histogram<u64>` per metric/endpoint, and they are merged with `add` on a 100 ms–1 s interval for the live UI.
  - Full per-request data streams to SQLite (as oha does with `--db-url`) or Parquet, for drill-down and git-friendly summaries.
- **Why not reqwest:** oha and rewrk go straight to hyper for control over connection pools, HTTP/2 multiplexing (`-p` streams per connection) and minimal overhead. reqwest is fine for "functional" execution of saved requests, but its pooling hides connection behaviour. Suggestion: share request *definitions* and a common templating/auth layer, but use a lean hyper path for load.
- **Allocator:** oha uses jemalloc only on non-MSVC targets. On Windows MSVC the system allocator applies, so consider `mimalloc` for Windows builds. This is an unbenchmarked suggestion.
- **Port math (Windows):** 16,384 ephemeral ports / 240 s TIME_WAIT ≈ **68 new client-closed connections per second sustained per (source IP → destination IP:port)** before exhaustion. Hence:
  - Default to keep-alive and connection reuse.
  - Warn when a profile forces a new connection per request.
  - Surface live counts of open, TIME_WAIT and errored sockets.
  - Prefer HTTP/2 multiplexing.
  - Because exhaustion on Windows breaks the whole machine's outbound networking, the app should cap new-connection rate by default.
- **Local mock target:** on Linux, loopback TIME_WAIT reuse is enabled by default (`tcp_tw_reuse=2`), which favours "load test my local mock." Running generator and mock on the same machine also shares CPU, so the app should report generator CPU saturation, as Locust does with its CPU-exhaustion warning.
- **File descriptors:** oha links `rlimit` on Unix, presumably to raise RLIMIT_NOFILE at startup. An embedded tester should do the same and show the effective limit in the UI.

### Gaps
- macOS defaults could not be verified because the search budget was exhausted:
  - Ephemeral range: `net.inet.ip.portrange.first/last`, commonly cited as 49152–65535.
  - Default soft fd limit: commonly 256.
  - `kern.maxfilesperproc`.
  - TIME_WAIT/MSL.
- The Linux default soft `nofile` limit (commonly 1024) and the hard-coded 60s TIME_WAIT were not verified from a primary source.
- No published benchmark compares the raw RPS of oha, rewrk, Goose, k6 and wrk on identical hardware in 2025–2026.
- Goose's HTTP client (believed to be reqwest) was not confirmed from the fetched pages.

---

## Q4. UX: what users like/dislike; recorders (browser/HAR → script); run comparison and baselines/regressions over time

### Takeaway
Users praise:
- Code-first DX with CI thresholds (k6).
- Out-of-the-box HTML reports (Gatling, JMeter).
- Live terminal and web dashboards (oha TUI, k6 web dashboard, Goose dashboard).

They complain about:
- JMeter's dated GUI and un-diffable XML.
- Locust's "live stats only" UI.
- k6's GUI living in a separate app (k6 Studio).

Recorders now offer HAR capture plus AI auto-correlation (k6 Studio 2.0). Run comparison and trend tracking are almost entirely locked behind paid SaaS: Grafana Cloud k6, Gatling Enterprise and Artillery Cloud. That leaves a clear opening for git-native, local run history and comparison.

### Cited Findings
- JMeter:
  - Dislikes: XML `.jmx` "hard to diff and review in Git", GUI "dated", resource-hungry. Likes: HTML report "comprehensive out of the box." k6 Studio is a "separate desktop app." Locust's web UI "shows live stats only" — [QAInsights, Jun 18 2026](https://qainsights.com/?p=18529)
  - CI gating is "clumsier than the others" — [ContextQA, Aug 2026](https://contextqa.com/blog/automated-load-testing-tools/)
- Gatling is described as the tool with built-in HTML reports by default — [Ranorex blog](https://www.ranorex.com/blog/load-testing-tools/); [ContextQA](https://contextqa.com/blog/automated-load-testing-tools/)
- Recorders:
  - **k6 Studio:**
    - Captures through a mitmproxy-based proxy into a HAR.
    - Rules handle correlation and parameterization, and the Validator replays one iteration — [k6-studio GitHub](https://github.com/grafana/k6-studio)
    - v2.0.0 (31 Jul 2026) made browser recording and Assistant-powered autocorrelation GA — [k6-studio releases](https://github.com/grafana/k6-studio/releases)
  - **Postman** reuses existing collections as the performance-test definition, with datafiles and datasets — [Postman docs](https://learning.postman.com/docs/tests-and-scripts/performance-testing/performance-test-configuration)
  - **Apidog** reuses existing test scenarios — [Apidog docs](https://docs.apidog.com/how-to-run-performance-test-in-apidog-748076m0)
  - **JMeter:** a HAR-to-JMeter conversion plugin exists (`io.github.vdaburon:har-convertor-jmeter-plugin`, versions 6.0 and 6.1) — [mvnrepository listing](https://badges.mvnrepository.com/artifact/io.github.vdaburon/har-convertor-jmeter-plugin/6.0/changes) (details not fetched)
- Live dashboards:
  - k6 web dashboard on :5665 with HTML export — [k6 docs](https://grafana.com/docs/k6/latest/results-output/web-dashboard/)
  - oha ratatui TUI — [oha](https://github.com/hatoo/oha)
  - Goose SSE dashboard with start/stop/set-users control — [Goose](https://github.com/tag1consulting/goose)
  - Postman streams real-time metrics; real-time metrics for CLI runs need a paid plan — [Postman docs](https://learning.postman.com/docs/tests-and-scripts/performance-testing/performance-test-configuration)
  - Vegeta's `plot` produces an interactive HTML latency-over-time chart — [Vegeta](https://github.com/tsenart/vegeta)
- Run comparison and baselines:
  - **Grafana Cloud k6:** baseline or starred reference run, compare 2 runs, or all runs of one script; same script only — [Grafana docs](https://grafana.com/docs/grafana-cloud/k6/analyze-results/test-comparison/)
  - **Gatling Enterprise:** trends over 10 runs and comparison of up to 5 runs across 11 metrics, paid only. AI verdicts added Jun 2026 — [Gatling docs](https://docs.gatling.io/reference/stats/trends/introduction/); [Gatling AI analysis](https://gatling.io/updates/ai-analysis-trends-comparison)
  - **Artillery Cloud:** lists "performance trends" among its features — [Artillery pricing](https://www.artillery.io/pricing)
  - **Postman:** "compare two performance runs," and renaming runs helps — [Postman docs](https://learning.postman.com/docs/tests-and-scripts/performance-testing/performance-test-configuration)
  - **drill:** thresholds compare against a stored previous run — [drill GitHub](https://github.com/fcsonline/drill)
- Cross-tool variance of up to 15–20% means baselines should stay within one tool — [QAInsights](https://qainsights.com/?p=18529)
- k6 v2.0 reworked the end-of-test summary into `compact` and `full` modes — [k6 v2.0.0](https://github.com/grafana/k6/releases/tag/v2.0.0); QAInsights calls the 2.0 summary "significantly improved" — [QAInsights](https://qainsights.com/?p=18529)

### Inferences
- Opportunities for "/dev/all":
  1. Store each run's summary (merged HDR histograms serialized as V2/base64, plus a JSON summary) as a file in the repo, e.g. `.devall/runs/<test>/<timestamp>-<gitsha>.json`.
  2. Make "compare with previous run / branch / commit" a local, free feature, which competitors put behind SaaS.
  3. Overlay latency-over-time charts with solid and dashed lines (Grafana's pattern).
  4. Flag regressions against a pinned baseline file (Gatling's pin-baseline workflow).
  5. Keep the test definition itself in a diffable text format (YAML/TOML/JSON), directly addressing JMeter's XML pain.
- The recorder should reuse the app's existing HTTP client and proxy: HAR import, then saved requests, then a load flow. Correlation rules should be extract (JSONPath/regex/header) then inject (`{{var}}`), matching k6 Studio and Artillery `capture`. A "Validate (1 VU, 1 iteration)" button with request/response inspection is table stakes.

### Gaps
- No primary Reddit or Hacker News threads were retrieved; the searches returned blog comparisons only. User sentiment is therefore drawn from 2026 comparison articles, not raw community discussion.
- Not verified this session: JMeter's built-in HTTP(S) Test Script Recorder, Gatling Recorder (HAR import), and Artillery recorder capabilities.

---

## Q5. Distributed mode: how tools coordinate agents; lightweight orchestration for a desktop app

### Takeaway
There are three patterns:
1. **Static partitioning of the load plan:** the k6 Operator splits a test across N pods via "execution segments."
2. **Live master/worker control with stat streaming:** Locust on port 5557; Goose Gaggle, now disabled.
3. **Vendor-managed cloud runners:** Gatling Enterprise load generators, Artillery on AWS Lambda, Fargate or Azure ACI, Postman cloud workers, and Grafana Cloud load zones.

For a desktop app, the lightest credible design ships the same engine as a headless worker container. The desktop app acts as coordinator: it sends each worker a plan plus its segment, then merges the HDR histograms the workers stream back.

### Cited Findings
- k6 Operator — [k6 docs: distributed tests](https://grafana.com/docs/k6/latest/testing-guides/running-distributed-tests/):
  - Uses a `TestRun` CRD, and `spec.parallelism` splits the workload using execution segments.
  - Scripts come from a ConfigMap (max "1,048,576 bytes (1 MiB)") or a PersistentVolume (`/test/`), in the same namespace.
  - `runner` sets env and `arguments` sets CLI flags.
  - `cleanup: post` is recommended with real-time output such as `experimental-prometheus-rw`.
  - Can be tried locally on kind or k3d.
- Locust — [Locust docs](https://docs.locust.io/en/stable/running-distributed.html):
  - The master runs the web UI only, with no users. Workers run users and send stats back.
  - Flags: `--master-bind-port`/`--master-port` (5557), `--expect-workers`.
  - Custom messages can pass between master and workers.
  - It recommends one worker per core.
  - 2.46.5 fixed the user-count metric in distributed runs, and 2.46.4 fixed the master stopping early when a worker quits — [Locust releases](https://github.com/locustio/locust/releases)
- Goose Gaggle — [Goose book](https://book.goose.rs/gaggle/overview.html):
  - The Manager starts and stops Workers and aggregates their metrics.
  - It compares a hash of the load-test rules to confirm each Worker matches the Manager; a mismatch can panic.
  - The same binary must be copied to every server.
  - Disabled since 0.17.0.
- Gatling: OSS is a single injector, and multi-machine runs require Enterprise. The Team plan includes 3 load generators — [ContextQA](https://contextqa.com/blog/automated-load-testing-tools/); [Gatling pricing](https://gatling.io/pricing)
- Artillery distributed runs — [Artillery releases](https://github.com/artilleryio/artillery/releases); [Artillery pricing](https://www.artillery.io/pricing):
  - Runs on AWS Lambda, AWS Fargate and Azure ACI, using SQS queues per `--aws-tags`.
  - Fargate workers self-terminate if the CLI vanishes for more than 180s.
  - Workers per test: 5 (Free), 25 (Team), unlimited (Business).
- NBomber's cluster mode requires the Enterprise license — [NBomber license](https://nbomber.com/docs/getting-started/license)
- Postman cloud runs — [Postman docs](https://learning.postman.com/docs/tests-and-scripts/performance-testing/performance-test-configuration):
  - Split load across Postman-managed workers.
  - Run from the US, or the EU for data residency.
  - Static IPs can be allowlisted.
  - Runs continue if you disconnect.
- Grafana Cloud k6 adds `k6 cloud run --local-execution`, which streams local results to the cloud, with log streaming in v2.2.0. It also has `k6 cloud load-zone list` — [k6 releases](https://github.com/grafana/k6/releases); [Grafana learning path](https://grafana.com/docs/learning-paths/establish-k6-baseline/save-share-results/)

### Inferences
- **Lightweight orchestration design:**
  - Publish a small headless binary or container (`devall-agent`) built from the same Rust engine.
  - The desktop coordinator connects over gRPC or WebSocket with mTLS and sends the resolved test plan: requests, data files, profile, and the segment index i/N.
  - Workers stream per-interval serialized HdrHistograms plus counters. The coordinator merges them with `Histogram::add` for live charts and correct global percentiles; averaging per-worker percentiles would be wrong.
  - Include a plan-hash check, as Goose does, so mismatched agents refuse to run.
  - Include a dead-man switch, as Artillery's 180s self-terminate does, so orphaned cloud workers stop on their own.
- **Deployment targets, from cheapest up:**
  1. `docker run` on any SSH-reachable host.
  2. A K8s Job with N parallel pods: generate a manifest and `kubectl apply`, mirroring the k6 Operator's `parallelism`.
  3. Later, cloud container services such as Fargate or ACI.
- Since all definitions are files in git, a worker can also fetch the plan by repo URL plus commit SHA. This makes distributed runs reproducible and auditable, which is an advantage over ConfigMap-size-limited approaches.

### Gaps
- Locust's wire protocol (believed to be ZeroMQ/msgpack) was not stated on the fetched page.
- k6 Operator caveats were not covered in the fetched page: threshold aggregation across pods and the end-of-test summary per pod.
- Gatling Enterprise's private-location and load-generator internals were not researched.

---

## Q6. Gaps in the market: unifying functional API tests + load tests + mocks from the same request definitions

### Takeaway
Only Postman (collections → performance tab) and Apidog (test scenarios → performance test) reuse functional request definitions for load, and both are weak as load testers:
- Closed-model VU profiles only.
- Apidog is capped at 100 VUs.
- Postman cloud is VUh-metered.
- No CO handling is advertised.
- Neither is git-native.

The dedicated load tools (k6, Gatling, Locust, Artillery) need separate scripts, and keep run history and comparison in paid SaaS. Insomnia has no load testing. No surveyed tool combines git-stored request files, a mock server, an open-model HDR-correct engine and local run-over-run comparison.

### Cited Findings
- Postman performance tests run a collection from the Collection Runner's Performance tab — [Postman resource usage](https://learning.postman.com/docs/administration/resource-usage/); [Postman perf config](https://learning.postman.com/docs/tests-and-scripts/performance-testing/performance-test-configuration)
  - Profiles are Fixed, Ramp up, Spike and Peak, all VU-based.
  - The local VU limit is hardware-bound.
  - Cloud runs require paid plans and VU hours.
- Apidog performance tests reuse "existing test scenarios," capped at "up to 100 virtual users" — [Apidog docs](https://docs.apidog.com/how-to-run-performance-test-in-apidog-748076m0)
- Insomnia has no built-in stress testing and recommends k6 or Locust — [Insomnia docs](https://docs.insomnia.rest/insomnia/stress-testing)
- JMeter's XML test plans are hard to diff and review in git — [QAInsights](https://qainsights.com/?p=18529)
- Run comparison and trends are paid features:
  - Gatling Enterprise only — [Gatling](https://gatling.io/features/run-trends-and-run-comparison)
  - Grafana Cloud k6 — [Grafana docs](https://grafana.com/docs/grafana-cloud/k6/analyze-results/test-comparison/)
  - Artillery Cloud — [Artillery pricing](https://www.artillery.io/pricing)
- Taurus unifies *load tools* (JMeter, Gatling, k6, Locust) under one YAML file, but not functional tests or mocks — [ContextQA](https://contextqa.com/blog/automated-load-testing-tools/)
- k6 Studio converts browser recordings into load scripts, but it is a separate, AGPL desktop app tied to Grafana Cloud for AI and cloud runs — [k6-studio](https://github.com/grafana/k6-studio); [k6-studio releases](https://github.com/grafana/k6-studio/releases)

### Inferences
- **Differentiators for "/dev/all"** (synthesis):
  1. **One request definition, three uses:** a saved request or flow is a functional test (assertions), a load scenario (assertions become checks, response times go into HDR) and a mock contract. "Load test against my local mock" becomes a one-click baseline: measuring the generator and client overhead against a near-zero-latency target is a useful calibration step most tools lack.
  2. **Git-native run history:** commit compact run summaries (HDR plus thresholds plus env and commit metadata) so "compare runs over time" is a `git log`, branch or PR diff, not a SaaS paywall. Thresholds in the same file give CI exit codes, as k6 and Postman CLI do.
  3. **Credible measurement:** default open model; raw vs CO-corrected display; dropped-iteration and port-exhaustion warnings. This beats Postman and Apidog, which are VU-only, and matches the "serious" tools.
  4. **Interop:**
     - Import: HAR, Postman collections, k6 scripts where feasible.
     - Export: a saved flow as a k6 script, so users can scale out on Grafana Cloud or the k6 Operator. Shelling out avoids the AGPL embedding issue.
     - Ship an optional containerized agent for distributed runs.
- Licensing caution: avoid linking AGPL k6 or k6 Studio code. Use MIT/Apache crates (hyper, tokio, hdrhistogram) and use Goose and oha as references. oha's license was not verified in this session.

### Gaps
- Postman and Apidog mock-server features were not verified in this session. It is commonly known that both offer mock servers, but no source was fetched to confirm how those mocks interact with their performance tests.
- No 2025–2026 survey or market report was found that quantifies demand for unified functional, load and mock tooling.
- oha's license (believed MIT) and drill's license were not captured from the fetched pages.

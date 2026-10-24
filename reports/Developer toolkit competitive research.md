# Build the joins single-purpose tools can't copy

/dev/all can only win by selling the connections between tools, because by October 2026 almost every module on the author's list is already table stakes somewhere. Postman 12 went git-native with one YAML file per request, Postman and Bruno both ship local mock servers, and nearly every API, database and Kubernetes client now carries an AI assistant and an MCP server. What no competitor offers is a single project folder where five things happen together: captured traffic becomes a WireMock-compatible stub; a webhook relay buffers events while the laptop sleeps and replays them with freshly re-signed signatures; a spec is checked against both the mocks and live traffic; a Kubernetes port-forward updates the HTTP environment; and a local model explains a failure by citing the project's own requests, logs and schemas. The money is real. At list prices, a commercial developer who pays separately for an API client, tunnel, mock host, database GUI and proxy spends about **$490 a year**, or about **$1,000** with load-testing and Kubernetes seats. A five-person team spends about **$5,700**. The features vendors paywall are git push, secret-manager integrations, response history, stateful mocks, whole-API chaos, rules-from-traffic and run comparison, and a local-first app can give all of them away at almost no marginal cost. The author's technical sketch mostly holds: Tauri 2.12 with a React frontend taken from wiremock-ui, one YAML file per project entity, secret references resolved from the OS keychain, and local models, either brought by the user through Ollama or LM Studio or run as a signed llama.cpp sidecar. That sidecar ships Metal builds on macOS and CPU plus Vulkan builds on Windows, and downloads CUDA only when it finds an NVIDIA GPU. The default models are all Apache-2.0: Qwen3.5 or Gemma 4 on 8–16 GB machines; Qwen3.8-27B (the newest Qwen open weights, August 2026) for general work and Qwen3-Coder-30B-A3B for autocomplete on 24 GB and up; and EmbeddingGemma-2 for search. macOS and Windows are first-class targets, and Linux stays second-tier. The one correction is Firebase. It suits accounts, licensing and store-and-forward webhook capture, but it cannot host a usable reverse tunnel; Cloudflare Workers with Durable Objects can, for about $5 a month. The build order follows from this. First fix the file format, the secrets model and the shared Rust core. Then ship the HTTP client, WireMock-compatible mocks and inline utilities as the wedge, where wiremock-ui already provides about 3,000 lines of tested logic and a finished design system. After that come the relay, webhook inbox, tunnel and traffic inspector, and last the load tester and the data and infrastructure viewers.

## Every planned module is already table stakes somewhere

### Files in git became the baseline in 2026, so trust is now what sets tools apart

The decisive 2026 event was Postman 12, released around March 2026 and described as "built from the ground up to be Git-native". It added a Collection 3.0 format that splits collections into individual YAML files, code-based local mock servers and an AI "Agent Mode" ([heise](https://heise.de/-11198271)). Postman now markets its Native Git as **"Local by default"**, with "one YAML file per request, per spec, per environment", on every plan ([Postman Native Git](https://www.postman.com/product/native-git/)). Bruno made OpenCollection YAML its default in February 2026, citing **4–5x faster parsing** than its old `.bru` format plus better tooling and AI support ([Bruno blog](https://blog.usebruno.com/making-yaml-the-default-in-bruno-v3.1)). Yaak mirrors its SQLite store to one YAML file per resource ([Yaak docs](https://yaak.app/docs/using-yaak/local-directory-git-sync)). The author's first goal, keeping every call and mock as a file in a repo ([README](../README.md)), is therefore necessary but no longer distinctive.

What still separates winners from losers is trust, and the category's defining events are breaches of it. Insomnia 8 forced logins and lost local data in 2023. Bruno moved from a one-time "Golden Edition" to subscriptions, which one blogger headlined "Bruno betrays" ([royathan.com](https://royathan.com/blog/the-api-client-hunt/)). Postman cut its Free plan to **one user** in March 2026 ([Postman Docs](https://learning.postman.com/docs/billing/about-plans)). A single press report says Nokia abandoned RapidAPI for Mac (formerly Paw) in September 2026 ([MacGeneration](https://www.macg.co/news/voir/259362/paw-interface-native-et-confortable-pour-curl)).

Infrastructure tools repeat the pattern. Lens charges organizations with more than $10M in revenue or funding ([Lens pricing](https://lenshq.io/pricing)). LocalStack began requiring auth tokens and archived its open-source repo on **March 23, 2026** ([GitHub localstack](https://github.com/localstack/localstack)). Azure Data Studio was retired on February 28, 2026 ([Microsoft](https://azure.microsoft.com/products/data-studio)), and the Kubernetes Dashboard was archived on January 21, 2026 ([GitHub kubernetes/dashboard](https://github.com/kubernetes/dashboard)).

The counterexample is Yaak. Its source is MIT-licensed and commercial use runs on an honor-system license, and it grew from **$160 to roughly $5k MRR** between December 2024 and September 2026 ([Yaak blog](https://yaak.app/blog)). For /dev/all, open formats, no forced account and a perpetual fallback license are features users choose on, not ethical extras.

### What each category charges and where it falls short

| Category | Leaders and list prices (Oct 2026) | Where they fall short | Opening for /dev/all |
|---|---|---|---|
| HTTP/API clients | Postman $0 / $9 / $19 per user; Insomnia $0 / $12 / $45; Bruno $0 / $6 / $11; Yaak $79/yr or $399 lifetime ([Postman](https://www.postman.com/pricing/); [Insomnia](https://insomnia.rest/pricing); [Bruno](https://www.usebruno.com/pricing); [Yaak](https://yaak.app/pricing)) | Secret-manager integrations only on enterprise tiers; git push and response history gated; Socket.IO and MQTT rare in git-native tools | Free secret providers, diffable response history, full protocol coverage, several collections per project |
| Mock servers | WireMock Cloud free for 1,000 calls/mo, then quote; Mockoon desktop free; Beeceptor $10–$99/mo ([WireMock](https://www.wiremock.io/pricing); [Beeceptor](https://beeceptor.com/pricing/)) | Stateful mocks, chaos and git sync are Enterprise-only; Mockoon lacks GraphQL, gRPC and SSE; WireMock JSON is verbose | WireMock-format engine with desktop UX, with state and chaos free |
| Webhook testers | webhook.site $9 / $19 / $69 per month; Hookdeck $0, then $39 / $499 ([webhook.site](https://docs.webhook.site/pro.html); [Hookdeck](https://hookdeck.com/pricing)) | 100-request caps and 3–7-day retention; no offline buffering; no replay that understands signatures | Unlimited local history, offline buffer, signature verify and re-sign |
| Tunnels | ngrok $0 / $10 / $20 plus usage; Cloudflare free; LocalXpose $8 ([ngrok](https://ngrok.com/pricing); [LocalXpose](https://localxpose.io/pricing)) | Quotas, warning pages, rotating URLs, no SSE on Cloudflare Quick Tunnels | Stable per-project URL; API traffic never sees a warning page |
| Traffic interception | Proxyman $89 one-time; Charles $50; Fiddler $7–$37/mo ([Proxyman](https://proxyman.com/pricing); [Fiddler](https://www.telerik.com/purchase/fiddler)) | Rules, breakpoints, rules-from-traffic and export are paid | Free capture → stub → spec pipeline |
| Load testing | k6 Cloud $0.15 per VU-hour; Gatling €89–€356/mo; Artillery $199–$499/mo ([Gatling](https://gatling.io/pricing); [Artillery](https://www.artillery.io/pricing)) | Run comparison is paid; load tests inside API clients use closed models and have caps | Open-model engine that corrects coordinated omission, with runs stored in git |
| Utility toolboxes | DevToys, IT-Tools, CyberChef free; DevUtils $29–$39 ([DevUtils](https://devutils.com/pricing)) | Commoditized and unaware of the user's project | Inline decoding everywhere from one shared detector registry |
| SQL/Redis/Mongo | TablePlus $99; DBeaver PRO $113–$510/yr; Studio 3T $499–$699/yr ([TablePlus](https://tableplus.com/pricing); [DBeaver](https://dbeaver.com/edition/); [Studio 3T](https://studio3t.com/buy/)) | JVM or Electron weight; NoSQL and AI behind paywalls; AI sends schemas to the cloud | Lightweight clients, saved queries as files, local natural-language-to-query |
| K8s/Docker/GCP | Lens $25–$50/seat; Aptakube $79/yr; Docker $9–$24/user ([Lens](https://lenshq.io/pricing); [Aptakube](https://aptakube.com/pricing); [Docker](https://www.docker.com/pricing/)) | Licenses gated on company revenue; no UI for the Pub/Sub emulator; auth-plugin failures | Port-forwards that fill in HTTP environments; GCP views that work against emulators first |

Adding these up shows why consolidation sells. A commercial individual paying for Postman Solo, ngrok Hobbyist, Beeceptor Individual, TablePlus and Proxyman spends about **$488** in year one. Adding k6 Pro and Lens Plus raises that to about **$1,016**. A five-person team's equivalent stack, including Hookdeck Team, Lens Pro and Docker Team, comes to roughly **$5,666** a year before AI assistants ([Postman](https://www.postman.com/pricing/); [ngrok](https://ngrok.com/pricing); [Beeceptor](https://beeceptor.com/pricing/); [TablePlus](https://tableplus.com/pricing); [Proxyman](https://proxyman.com/pricing); [Hookdeck](https://hookdeck.com/pricing); [Lens](https://lenshq.io/pricing); [Docker](https://www.docker.com/pricing/)). These totals are sums of list prices, not measured spending. The research found no survey of how many dev-tool subscriptions developers actually hold.

### API clients all moved to YAML, but the basics are still paywalled

By late 2026 Postman, Insomnia, Bruno, Yaak, Hoppscotch and Kreya all support REST, GraphQL, WebSocket and gRPC, and most add mocks, a CLI runner and some AI or MCP hook ([heise](https://heise.de/-11198271); [Insomnia releases](https://github.com/Kong/insomnia/releases); [Bruno releases](https://github.com/usebruno/bruno/releases)). The real differences are in less common protocols and in what sits behind a paywall. Among the verified tools, MQTT exists only in Hoppscotch and the ATAC terminal client, and Socket.IO mainly in Hoppscotch and Insomnia. Hoppscotch documents no git-friendly file layout ([Hoppscotch](https://github.com/hoppscotch/hoppscotch); [ATAC](https://github.com/Julien-cpsn/ATAC); [Insomnia releases](https://github.com/Kong/insomnia/releases)). Socket.IO is among Bruno's four most-upvoted open requests ([Bruno issues](https://github.com/usebruno/bruno/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)).

Kreya sets the bar for auth and transports: AWS SigV4, mTLS, Kerberos, NTLM, JWT Profile, HTTP/3, Unix sockets and Windows named pipes. Its free tier, however, excludes scripting, tests and history ([Kreya](https://kreya.app/); [Kreya pricing](https://kreya.app/pricing/)).

Secret-manager integrations are the clearest paywall. Postman requires Enterprise plus an Advanced Security Administration add-on ([Postman Vault docs](https://learning.postman.com/docs/sending-requests/postman-vault/postman-vault-secrets)). Insomnia's vault integrations need Enterprise at $45 per user per month ([Insomnia pricing](https://insomnia.rest/pricing)). Bruno puts them in its Ultimate tier, and it also charges for git push, branches and merge-conflict resolution ([Bruno pricing](https://www.usebruno.com/pricing)).

The unmet needs repeat across feedback boards. Yaak's top requests are multi-tab support (66 votes), request scripting (64) and response history (47), with OpenAPI export at 37 ([Yaak feedback](https://yaak.app/feedback)). Bruno users ask for nested environments, HAR import, MCP integration and a `.env` file per environment ([Bruno issues](https://github.com/usebruno/bruno/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)). Insomnia users want temporary tabs that don't auto-save, jq filters and collections kept as plain local files ([Insomnia issues](https://github.com/Kong/insomnia/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)). One developer's January 2025 survey of API clients named **comparing past responses** as a must-have that was missing or paywalled almost everywhere ([royathan.com](https://royathan.com/blog/the-api-client-hunt/)).

Merge hygiene is the remaining technical gap. Bruno writes an integer `seq` into every request file, so a single drag-reorder rewrites many files ([Bruno docs](https://docs.usebruno.com/opencollection-yaml/overview)). Insomnia admits its Git Sync "sometimes writes extra metadata" that shows up as spurious changes ([Kong Git Sync](https://developer.konghq.com/insomnia/git-sync/)).

Yaak is the closest competitor in both stack and philosophy: Tauri, Rust and React, local-first, an MCP server and a git UI. One user measured it at about 230 MB of RAM against Bruno's 500 MB ([royathan.com](https://royathan.com/blog/the-api-client-hunt/)). Yaak's gaps, no scripting and response history still in progress, are openings for /dev/all.

### Mock servers: WireMock owns the format, but nobody owns the desktop

WireMock is the de facto stub format. It stores one JSON file per stub and has a rich set of matchers, Handlebars templating, scenarios, recording and an admin API described in OpenAPI ([WireMock stubbing](https://wiremock.org/docs/stubbing/); [admin API](https://wiremock.org/docs/standalone/admin-api-reference/)). Its hosted product gates the valuable parts. WireMock Cloud's free tier allows **1,000 calls a month and 3 mock APIs**, and stateful mocking, chaos, git sync and MCP are Enterprise-only ([WireMock pricing](https://www.wiremock.io/pricing)). WireMock's own comparison lists CSV or database-backed data sources and whole-API chaos as Cloud features that the OSS version lacks ([WireMock comparison](https://docs.wiremock.io/concepts/comparison)).

Mockoon has the best local desktop UX: CRUD over data buckets, Faker templating, recording and OpenAPI import ([Mockoon features](https://mockoon.com/features/)). But its rules are shallow, and its most-upvoted open issues ask for GraphQL, a web UI, gRPC, HAR import, an API for defining routes and SSE ([Mockoon issues](https://github.com/mockoon/mockoon/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)). Its paid tier was reported two ways in the research, as €10 per self-hosted license or $15–$29 per cloud seat, depending on the page snapshot ([Mockoon pricing](https://mockoon.com/pricing/)).

The other mock tools each cover only part of the need. Beeceptor's free tier allows **50 requests a day** ([Beeceptor pricing](https://beeceptor.com/pricing/)). Prism is OpenAPI-only and stateless ([Prism](https://github.com/stoplightio/prism)). Microcks leads on spec-driven mocking across many protocols ([Microcks](https://microcks.io/)). The closest match to the planned /dev/all mock module is mockd. It is a single Go binary that imports WireMock, Mockoon and HAR, mocks seven protocols, and ships chaos profiles and an MCP server, but it has only about 150 stars and no desktop UI ([mockd](https://github.com/getmockd/mockd)).

WireMock's own users complain about verbosity. Open requests include YAML stub support (#2411), saving recorded JSON as a readable `jsonBody` instead of an escaped string (#1399) and stub TTLs (#1871) ([WireMock issues](https://github.com/wiremock/wiremock/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)). Its CONNECTION_RESET_BY_PEER fault "will most likely hang" on Windows ([WireMock faults](https://wiremock.org/docs/simulating-faults/)).

The existing GUIs are thin front-ends over the admin API. The most complete, holomekc's fork, has 138 stars and only an experimental state-machine view ([holomekc/wiremock](https://github.com/holomekc/wiremock)). None of them lets you edit stubs as files in git and hot-push them to a running instance through `/__admin/mappings/import` with `deleteAllNotInImport` ([WireMock stubbing](https://wiremock.org/docs/stubbing/)). The research also found no Rust crate that serves WireMock's stub format natively. The author's wiremock-ui already fills half of this gap.

### Webhook testers and tunnels sell retention, quotas and stable URLs

Webhook testers make money from the same three levers: retention, quotas and stable URLs. webhook.site's free URLs stop accepting requests after **100 requests**, **expire after 7 days** and can be read by anyone who has the ID. Paid plans cost $9, $19 and $69 a month ([webhook.site docs](https://docs.webhook.site/); [plans](https://docs.webhook.site/pro.html)). Hookdeck's Console needs no account, but its Event Gateway keeps only **3 days** of history on the free tier and 7 days at $39 a month ([Hookdeck pricing](https://hookdeck.com/pricing)). smee.io stores nothing, so events that arrive while the client is offline are lost ([smee.io](https://github.com/probot/smee.io)). On free plans, ngrok's Full Capture truncates bodies at **10 KB** and keeps them for 24 hours ([ngrok traffic inspection](https://ngrok.com/docs/obs/traffic-inspection)).

Tunnels follow the same pattern:

- **ngrok:** the free plan caps usage at 1 GB and 20,000 HTTP requests a month and puts a warning page in front of browsers. Hobbyist costs $10 a month ([ngrok pricing](https://ngrok.com/pricing)).
- **Cloudflare Quick Tunnels:** free, but the URL is random, there is no uptime guarantee, in-flight requests are capped at 200, and they **cannot carry Server-Sent Events** ([Cloudflare](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/)).
- **Tailscale Funnel:** exposes only ports 443, 8443 and 10000, on `ts.net` domains ([Tailscale](https://tailscale.com/kb/1223/funnel)).

Developers' complaints center on quotas, URLs that rotate, setup friction ("50 lines of TOML") and vendors that block shared tunnel domains ([HN: tnnl](https://news.ycombinator.com/item?id=47088419); [HN: rustunnel](https://news.ycombinator.com/item?id=47425918)).

Provider signature schemes turn this from a plumbing problem into a product opportunity:

- Stripe, GitHub, Shopify, Slack and the Standard Webhooks spec all sign the **raw body**.
- Twilio also signs the exact public URL, so re-encoding the URL breaks validation.
- Stripe and Slack reject events more than **five minutes** old.
- Shopify allows only five seconds for the whole request before it retries.

Sources: [Stripe](https://docs.stripe.com/webhooks); [Twilio](https://www.twilio.com/docs/usage/webhooks/webhooks-security); [Shopify](https://shopify.dev/docs/apps/build/webhooks/subscribe/https); [Slack](https://docs.slack.dev/authentication/verifying-requests-from-slack); [Standard Webhooks](https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md).

Stripe itself names two causes as the most common verification failures: mixing up the CLI and Dashboard secrets, and frameworks that change the raw body ([Stripe signature errors](https://docs.stripe.com/webhooks/signature)). A relay therefore has to forward bytes exactly, expose the original public URL, and acknowledge quickly while the desktop is offline. It should also do what no surveyed tool does: **re-sign replayed events with a fresh timestamp** using a test secret, so replays still pass after the tolerance window has expired.

### Traffic interception charges for the rules that make capture useful

Commercial proxies charge for exactly the steps that turn captured traffic into something you can act on:

- **HTTP Toolkit:** the free tier intercepts and inspects. Mock rules, "rules from real traffic", edit-and-resend and HAR or code export are Pro features ([HTTP Toolkit pricing](https://httptoolkit.com/pricing/)).
- **Proxyman:** a **$89** one-time license. Breakpoints, Map Local/Remote and scripting are excluded from the trial ([Proxyman](https://proxyman.com/pricing)).
- **Charles:** costs $50 ([Charles](https://www.charlesproxy.com/buy/)).
- **Fiddler Everywhere:** $7–$37 a month, with rules and breakpoints excluded from the Lite tier ([Telerik](https://www.telerik.com/purchase/fiddler)).

mitmproxy remains the free, scriptable option. It can capture traffic per process on Windows, Linux and macOS, and has a WireGuard mode for mobile devices ([mitmproxy modes](https://docs.mitmproxy.org/stable/concepts/modes/)). HTTP Toolkit's most-requested features map directly onto a combined inspector and mock tool: scripted mock rules, gRPC decoding with a chosen `.proto` file, WebSocket breakpoints, auto-saving matched traffic to HAR, and request tagging ([HTTP Toolkit issues](https://github.com/httptoolkit/httptoolkit/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)). Rust already has the building blocks in hudsucker 0.25, which generates its CA certificates with rcgen ([hudsucker](https://docs.rs/hudsucker/latest/hudsucker/)).

Tools that turn traffic into a spec are fragmented:

- **mitmproxy2swagger**, the most used, is a manual two-pass process, and its `--examples` flag can leak tokens into the spec ([mitmproxy2swagger](https://github.com/alufers/mitmproxy2swagger)).
- **Optic**'s open-source repo was **archived on January 12, 2026**, after Atlassian acquired the company ([Optic](https://github.com/opticdev/optic)).
- **Redocly**'s experimental `generate-spec` accepts AI refinement only if the result keeps the observed paths, methods and status codes and passes ruleset validation ([Redocly](https://redocly.com/docs/cli/commands/generate-spec)).
- **Postman Insights**, its traffic-based feature, is Enterprise-only ([Postman pricing](https://www.postman.com/pricing/)).

The niche Optic left is open: local spec inference from traffic that redacts sensitive values, validates its output, and shows a diff against the spec already committed.

### Load testers hide run comparison behind SaaS and mismeasure tail latency

The serious load-testing engines are code-first tools with paid SaaS tiers. Grafana Cloud k6 bills **$0.15 per VU-hour** ([Grafana](https://grafana.com/docs/grafana-cloud/cost-management-and-billing/analyze-costs/reduce-costs/performance-testing-costs/calculator/)). Gatling Enterprise starts at €89 a month ([Gatling pricing](https://gatling.io/pricing)) and Artillery Cloud at $199 ([Artillery pricing](https://www.artillery.io/pricing)). Run comparison and trend tracking, which teams need to catch regressions, are almost entirely behind those paywalls ([Gatling trends](https://gatling.io/features/run-trends-and-run-comparison)).

The API clients that reuse saved requests for load testing are weak at it. Postman offers only profiles based on a fixed number of virtual users, a closed model ([Postman performance](https://learning.postman.com/docs/tests-and-scripts/performance-testing/performance-test-configuration)). Apidog caps tests at **100 virtual users** ([Apidog](https://docs.apidog.com/how-to-run-performance-test-in-apidog-748076m0)). Insomnia has no load testing at all ([Insomnia](https://docs.insomnia.rest/insomnia/stress-testing)).

Closed models also measure latency wrongly, a problem called coordinated omission. When the server stalls, a closed-loop tester stops sending, so the slowest requests are never recorded. wrk2's README shows a 1.4-second stall reported as a 6 ms p99 instead of the correct 1.27 s, an understatement of about **200x** ([wrk2](https://github.com/giltene/wrk2)). k6 fixes this with arrival-rate executors ([k6 docs](https://grafana.com/docs/k6/latest/using-k6/scenarios/concepts/open-vs-closed/)). oha corrects for it only when a target rate is set ([oha](https://github.com/hatoo/oha)). Rust's `hdrhistogram` crate provides `record_correct` for closed-model runs ([hdrhistogram](https://docs.rs/hdrhistogram/latest/hdrhistogram/)).

A desktop load tester also faces a platform hazard on Windows. Windows has 16,384 ephemeral ports and a four-minute TIME_WAIT, and running out of ports breaks *all* outbound networking on the machine, including RDP and domain sign-in ([Microsoft Learn](https://learn.microsoft.com/en-us/troubleshoot/windows-client/networking/tcp-ip-port-exhaustion-troubleshooting)). That works out to roughly 68 new client-closed connections per second per destination before trouble starts.

Licensing narrows the options further. k6 and k6 Studio are AGPL-3.0, so a closed-source app should launch them as separate programs rather than embed them ([QAInsights](https://qainsights.com/?p=18529)). oha's tokio, hyper and rustls stack is the proven Rust approach to copy ([oha Cargo.toml](https://raw.githubusercontent.com/hatoo/oha/master/Cargo.toml)).

### Utility toolboxes are free, commoditized and unaware of the user's project

The toolbox category is mature and mostly free:

- **DevToys** ships 30 offline tools and detects what is on the clipboard ("Smart Detection") ([DevToys](https://devtoys.app/)).
- **IT-Tools** has about 86 tools under the GPL ([IT-Tools](https://github.com/CorentinTh/it-tools)).
- **CyberChef** offers 300+ operations that you chain into drag-and-drop recipes.
- **DevUtils** sells 47 tools for **$29–$39** one-time ([DevUtils pricing](https://devutils.com/pricing)).

Raycast install counts show where demand concentrates: Format JSON at about 103,000 installs, UUID Generator at 36,500 and Diff Checker at 22,100 ([Raycast Store](https://www.raycast.com/store/category/developer-tools)).

CyberChef's "Magic" feature is the reference design for detecting what a value is:

1. Each operation declares regex detectors for inputs it can decode.
2. Matching operations run speculatively, branching recursively up to a set depth.
3. Each branch is scored on magic bytes, UTF-8 validity, entropy and how closely it resembles natural language.

All of this runs in a background thread, and when it finds something the user can append the suggested operations to the recipe with one click ([CyberChef wiki](https://github.com/gchq/CyberChef/wiki/Automatic-detection-of-encoded-data-using-CyberChef-Magic)). CyberChef's 2026 changelog also lists repeated XSS and ReDoS fixes, a warning for any app that renders untrusted output in a webview ([CyberChef changelog](https://github.com/gchq/CyberChef/blob/master/CHANGELOG.md)).

The best single-purpose tools set specific bars:

- **Regex:** regex101 supports eight regex flavors, unit tests and, in Pro, a debugger ([regex101](https://docs.regex101.com)). Rust's `regex` crate guarantees linear time but supports **no look-around and no backreferences**, so a faithful PCRE or JavaScript mode needs a second engine ([regex crate](https://docs.rs/regex/latest/regex/)).
- **Diff:** jd diffs JSON semantically, can treat arrays as sets for chosen paths, and outputs RFC 6902 patches ([jd](https://github.com/josephburnett/jd)).
- **Fake data:** Mockaroo charges $60 to $7,500 a year for realistic generated data ([Mockaroo](https://www.mockaroo.com/pricing)). Faker libraries produce reproducible output only when seeded and given a fixed reference date ([Faker docs](https://fakerjs.dev/guide/usage.html)).
- **Logs:** lnav runs SQL over 70+ log formats that it detects automatically ([lnav](https://lnav.org/features)). klogg handles files over 10 GB with SIMD-accelerated regex ([klogg](https://github.com/variar/klogg)). Grafana's Logs Drilldown clusters log lines into patterns without the user writing a query ([Grafana](https://github.com/grafana/logs-drilldown)).
- **JWT:** the strongest argument is privacy. Pasting production tokens into jwt.io hands credentials to a third party ([DEV Community](https://dev.to/rmb_37/stop-pasting-jwts-into-jwtio-12kn)). Newer decoders fetch JWKS keys automatically to verify signatures ([Duende](https://duendesoftware.com/blog/20251120-inspect-and-validate-json-web-tokens-with-the-duende-jwt-decoder)).

None of these tools knows anything about the user's project, traffic or logs.

### Data and infrastructure clients suffer from bloat, paywalled NoSQL support and cloud-only AI

SQL clients fall into three groups: native paid apps, heavy JVM IDEs and Electron apps with a free core and paid extras. TablePlus costs **$99** per device, and its free tier allows only two open tabs ([TablePlus](https://tableplus.com/pricing)). DBeaver Community is free, but MongoDB, Redis and cloud databases are PRO-only ([DBeaver](https://dbeaver.io/)). Beekeeper's "AI Shell" starts at its Professional tier ([Beekeeper pricing](https://www.beekeeperstudio.io/pricing)). DbGate's AI chat is in Premium, at $12 a month ([DbGate pricing](https://www.dbgate.io/pricing/)). Secondary comparisons describe DBeaver using 1–2 GB of RAM at idle ([DbVis](https://www.dbvis.com/thetable/best-dbeaver-alternatives-of-2025/)).

Redis and Mongo tools show the AI privacy problem plainly. Redis Copilot may send index schemas and sample keys to OpenAI ([Redis Copilot FAQ](https://redis.io/docs/latest/develop/tools/insight/copilot-faq/)). Compass's natural-language queries send schema details to Azure OpenAI ([MongoDB docs](https://www.mongodb.com/docs/compass/query-with-natural-language/)). Studio 3T costs $499–$699 per user per year and offers "no on-premises model option" ([Studio 3T](https://studio3t.com/buy/)). Another Redis Desktop Manager, which is free, MIT-licensed and has 34,800 stars, shows that a lightweight Redis browser is achievable ([ARDM](https://github.com/qishibo/AnotherRedisDesktopManager)). Usage data sets the priority: in Stack Overflow's 2026 survey, 57.9% of respondents used PostgreSQL, 28.3% Redis and 19.0% MongoDB ([SO 2026](https://survey.stackoverflow.co/2026/technology)).

On the Kubernetes side, Lens charges larger organizations $25–$50 per seat ([Lens](https://lenshq.io/pricing)). Free alternatives cover the daily essentials: MIT-licensed Freelens, Headlamp (the official successor to the Kubernetes Dashboard) and k9s ([Freelens](https://github.com/freelensapp/freelens); [Kubernetes blog](https://kubernetes.io/blog/2026/06/01/dashboard-to-headlamp/); [k9s](https://github.com/derailed/k9s)). Two Tauri apps show the stack works for this kind of tool. Aptakube is free for clusters of five nodes or fewer and costs $79 a year otherwise ([Aptakube](https://aptakube.com/pricing)). kftray is a Rust and Tauri port-forward manager that runs without kubectl, inspects and replays HTTP traffic, and **shares configs via GitHub** ([kftray](https://github.com/hcavarsan/kftray)).

Docker Desktop is paid for companies with more than 250 employees or more than $10M in revenue ([Docker](https://docs.docker.com/subscription/desktop-license/)). A viewer, however, needs only the Docker Engine API, which the Rust crate `bollard` wraps for both Docker and Podman ([bollard](https://crates.io/api/v1/crates/bollard)).

GCP has the clearest gap. The Pub/Sub emulator "doesn't support the Google Cloud console or `gcloud pubsub` commands", Rust clients need code changes to target it ([Pub/Sub emulator](https://docs.cloud.google.com/pubsub/docs/emulator)), and the community-built UIs are all web apps ([Google forum](https://discuss.google.dev/t/pubsub-emulator-ui/145303)).

The practical trap across all of these tools is authentication. Kubeconfig exec plugins such as `gke-gcloud-auth-plugin` still have to be launched as separate processes. Apps started from a desktop GUI fail when they don't inherit the shell's PATH, or when the plugin is installed only inside WSL ([Kubernetes discuss](https://discuss.kubernetes.io/t/unable-to-connect-to-the-server-getting-credentials-exec-executable-gke-gcloud-auth-plugin-not-found/23004); [TabNews](https://www.tabnews.com.br/danieldia/usando-o-gke-gcloud-auth-plugin-do-wsl-dentro-do-lens)).

### AI ships everywhere, but it is metered and only partly trusted

Every competitor now ships AI:

- **Postman's Agent Mode** runs on metered credits: 400 a month on Solo, with overage at $0.05 each ([Postman pricing](https://www.postman.com/pricing/)).
- **Insomnia 12** generates mocks from prompts, specs or live responses, and accepts a local `.gguf` model file. Its docs warn that models under 10B parameters produce less accurate output ([Insomnia AI docs](https://developer.konghq.com/insomnia/ai-in-insomnia/)).
- **Bruno 4** uses the user's own API keys for AI calls ([EveryDev](https://www.everydev.ai/tools/bruno-ai)).
- **Hoppscotch** requires the user to review and accept every AI edit ([Hoppscotch docs](https://docs.hoppscotch.io/documentation/features/ai-features)).
- **K8sGPT** runs read-only analyzers with fixed rules and uses an optional LLM, including a local Ollama model, only to explain what they find ([K8sGPT](https://github.com/k8sgpt-ai/k8sgpt)).

Developers trust AI conditionally. In Stack Overflow's 2026 survey, **48% trust AI when they can easily validate it**, 93% say citing sources matters, and 13% avoid AI at work because of privacy or security ([SO 2026 AI](https://survey.stackoverflow.co/2026/ai)).

MCP is the new way for these tools to connect to AI agents. About 70% of API developers know it and 10% use it regularly ([Postman State of the API](https://www.postman.com/state-of-api/)). Postman, Hoppscotch, Yaak, DataGrip and Lens all expose MCP servers. The opening left for /dev/all is AI that runs locally by default, works from the project's own files and data, and validates its output before the user accepts it.

## Twenty-two features the original list leaves out

The author's list names the modules ([README](../README.md)). The research points to additions in seven areas:

- a traffic inspector, which is the missing piece between the tunnel and the mock server;
- broader protocol and authentication support;
- a testing and contract layer;
- integration with AI agents through MCP;
- trust features: secret providers, scanning, redaction and safety modes;
- project-level operations: port-forwards, SSH tunnels and a status dashboard;
- the long tail of utilities that matter for API work.

The table below lists each addition with the evidence that it matters and the modules it connects.

| Addition | Evidence it matters | What it connects to |
|---|---|---|
| **Traffic inspector**: MITM proxy, local CA, breakpoints, Map Local/Remote | Paid features at HTTP Toolkit Pro and Proxyman ([HTTP Toolkit](https://httptoolkit.com/pricing/); [Proxyman](https://proxyman.com/pricing)) | Captured flows become stubs, specs and load scenarios |
| **Response history stored as files**, with diff and snapshot tests | Yaak's third-most-requested feature (47 votes); snapshot tests are Pro-only in Kreya ([Yaak feedback](https://yaak.app/feedback); [Kreya pricing](https://kreya.app/pricing/)) | Semantic diff, drift detection, auto-generated assertions |
| **GraphQL introspection, gRPC reflection, SSE (including LLM streams), Socket.IO, MQTT** | Socket.IO is a top Bruno request; among surveyed clients only Hoppscotch and ATAC support MQTT ([Bruno issues](https://github.com/usebruno/bruno/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc); [Hoppscotch](https://github.com/hoppscotch/hoppscotch)) | Mock engine, inspector decoders |
| **Full auth support**: OAuth2/PKCE with auto-refresh and a localhost callback, SigV4, mTLS, NTLM, Digest | Kreya's coverage sets the bar; Yaak's hosted OAuth redirect sends callbacks through `oauth.yaak.app` ([Kreya](https://kreya.app/); [Yaak security](https://yaak.app/docs/getting-started/security-and-privacy)) | JWT tool, environments |
| **OpenAPI round-trip**: import, watch for changes, export, Spectral-style linting | Yaak requests for OpenAPI export (37 votes) and URL sync (22); Bruno caps free syncs at 5 a month ([Yaak feedback](https://yaak.app/feedback); [Bruno pricing](https://www.usebruno.com/pricing)) | Mocks, fake data, drift checks |
| **Contract and drift checks** across spec, mock and live traffic, with a Prism-style validation proxy and Pact import/export | 17% of teams do contract testing vs 67% functional testing; Optic is archived ([Postman SOTA](https://www.postman.com/state-of-api/); [Optic](https://github.com/opticdev/optic)) | Inspector, mocks, CI |
| **Collection runner and headless CLI with JUnit output**, plus a Docker image for running mocks in CI | Standard in Bruno, Insomnia and Hurl ([Hurl](https://hurl.dev/)) | Every module |
| **`.http` and Hurl file compatibility** | Common JetBrains and REST Client format ([JetBrains](https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html)) | HTTP client, CI |
| **MCP server with tiered permissions, plus an MCP client and inspector** | Yaak users ask for an MCP client; WireMock gates MCP to Enterprise and Fiddler to Pro and above ([Yaak post](https://yaak.app/feedback/posts/native-mcp-client-for-testing-and-debugging-mcp-servers); [WireMock](https://www.wiremock.io/pricing)) | Every module, for coding agents |
| **Free secret-provider integrations** (1Password, Bitwarden, Vault, AWS, GCP), plus secret scanning and redaction | Enterprise-only at Postman, Insomnia and Bruno; Hurl redacts secrets from its reports ([Postman Vault](https://learning.postman.com/docs/sending-requests/postman-vault/postman-vault-secrets); [Hurl releases](https://github.com/Orange-OpenSource/hurl/releases)) | Git commits, exports, AI calls |
| **Mock OIDC/JWKS endpoint and test-token signer** | Decoders now fetch JWKS keys automatically, so verifying a token needs a key source ([Duende](https://duendesoftware.com/blog/20251120-inspect-and-validate-json-web-tokens-with-the-duende-jwt-decoder)) | JWT tool, mocks, load tests |
| **Port-forward manager** that auto-reconnects and feeds environments | Headline feature in Headlamp 0.46 and kftray ([Headlamp](https://github.com/kubernetes-sigs/headlamp/releases); [kftray](https://github.com/hcavarsan/kftray)) | K8s viewer, HTTP client |
| **SSH tunnels and connection safety modes**: read-only, protected, production highlighting | TablePlus Safe mode, k9s `--readonly` ([TablePlus](https://tableplus.com/); [k9s](https://github.com/derailed/k9s)) | DB clients, K8s, wiremock-ui's environment chips |
| **GCP views that work against emulators first**: Pub/Sub, Cloud Storage, Logging tail, Secret Manager import | The Pub/Sub emulator has no UI ([Pub/Sub emulator](https://docs.cloud.google.com/pubsub/docs/emulator)) | Log viewer, secrets, fake data |
| **Embedded DuckDB to run SQL over logs, responses and CSV** | Dozzle and lnav ship SQL over logs ([Dozzle](https://dozzle.dev/); [lnav](https://lnav.org/features)) | Logs, responses, CSV, load results |
| **cURL ↔ code conversion and JSON → types** | curlconverter supports about 30 targets, quicktype 21 languages ([curlconverter](https://github.com/curlconverter/curlconverter); [quicktype](https://github.com/glideapps/quicktype)) | HTTP client, mocks |
| **jq / JSONPath / JMESPath playground over live responses** | Insomnia #777 asks for jq ([Insomnia issues](https://github.com/Kong/insomnia/issues?q=is%3Aissue+is%3Aopen+sort%3Areactions-%2B1-desc)) | Assertions, value extraction, logs |
| **Clipboard detection, command palette and CyberChef-style recipes** | DevToys Smart Detection, CyberChef Magic ([DevToys](https://devtoys.app/); [CyberChef wiki](https://github.com/gchq/CyberChef/wiki/Automatic-detection-of-encoded-data-using-CyberChef-Magic)) | Inline decoding in every view |
| **Project-wide chaos and network-condition presets** | Whole-API chaos is a WireMock Cloud feature, missing from OSS ([WireMock comparison](https://docs.wiremock.io/concepts/comparison)) | Mocks, tunnel, load tests |
| **LLM endpoint playground**: streaming view, token and latency comparison | Postman compares models side by side; Yaak renders LLM streams readably ([Postman AI](https://learning.postman.com/docs/postman-ai/overview); [Yaak 2026.5](https://newreleases.io/project/github/mountain-loop/yaak/release/v2026.5.0)) | HTTP client, local LLM layer |
| **Project status dashboard**: tunnels, running mocks, last webhook received, port-forwards, token expiry | Inferred from the cross-tool workflows; no direct source | Everything |
| **Network utilities**: fetch a TLS chain into the X.509 decoder, DNS lookup, port check, next cron runs in a chosen time zone | Long-tail candidates; demand not verified in this research | Utilities, HTTP client error diagnosis |

The traffic inspector is the most important addition, for two reasons. First, it supplies captured traffic to three of the workflows that connect modules: capture → mock, traffic → spec, and recording a load scenario. Second, it covers outbound traffic, which the planned tunnel does not. A tunnel only helps when traffic flows into the laptop, while an inspector catches what the user's app sends out.

The research also argues against a few things. /dev/all should not replace the container runtime that Docker Desktop, OrbStack or Podman provide. It should connect to whichever socket exists, as `bollard` already does ([bollard](https://docs.rs/crate/bollard/0.21.1)). For Helm, `docker compose` and `cloud-sql-proxy`, it should call the existing tools rather than reimplement them. Three-way merge and an embedded `mongosh` shell are out of scope for a utility toolkit.

## The connections between tools are the product

The cross-tool workflows are defensible for a structural reason. A single-purpose competitor cannot copy the webhook loop or the cluster-to-environment binding without owning at least two modules, such as a relay plus an inspector, or a cluster viewer plus an HTTP client.

These connections become cheap if four shared foundations exist from the start:

1. **One canonical request/response format.** HTTP client history, inspector captures, the webhook inbox, the mock request journal and HAR imports all write to it. wiremock-ui's `request-to-stub` and HAR modules already work on such a shape ([request-to-stub.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/request-to-stub.ts); [har.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/har.ts)).
2. **One schema and data engine.** It turns OpenAPI or JSON Schema into seeded fake data and feeds mock responses, load-test request bodies and database seeds.
3. **One event store.** Every runtime module writes to it, tagged with correlation IDs and timestamps, and the log viewer reads from it.
4. **One registry of typed values, detectors and operations.** The same registry drives clipboard detection, inline decoding, recipes, the CLI and MCP tools.

Designed in from the start, each cross-tool workflow is a small feature. Added later, each one becomes a data migration. The table below lists the workflows the evidence supports, roughly in order of value.

| Workflow | The chain inside one project | What it replaces today | Evidence |
|---|---|---|---|
| Webhook loop | Relay URL → inbox → verify signature → forward to localhost → edit, re-sign and replay → save as fixture or test | Provider CLI or ngrok, plus webhook.site or Hookdeck, plus an HTTP client | Stripe documents this exact ngrok/CLI/resend chain ([Stripe](https://docs.stripe.com/webhooks)) |
| Capture → mock → replay | Inspector or history → select flows → WireMock stub with a readable `jsonBody` → Faker template → regression test | Proxyman or HTTP Toolkit, plus WireMock or Mockoon, plus a client | WireMock recording; Keploy built a company on record-and-replay ([WireMock](https://wiremock.org/docs/record-playback/); [Keploy](https://keploy.io/)) |
| Request → mock → randomize | Saved request → stub → data generated from the inferred schema | Manual stub writing | The README's own flagship example ([README](../README.md)) |
| Drift between spec, mock and traffic | OpenAPI spec in the repo → generated mocks and requests → live traffic validated against both | Prism, plus Optic (archived), plus Spectral | Only 17% of teams do contract testing ([Postman SOTA](https://www.postman.com/state-of-api/)) |
| Traffic → OpenAPI | Cluster paths and merge schemas → LLM adds names and descriptions → lint → diff against the committed spec | mitmproxy2swagger, Postman Insights (Enterprise) | Redocly's pattern of accepting AI output only after validation ([Redocly](https://redocly.com/docs/cli/commands/generate-spec)) |
| Cluster → environment | K8s port-forward or Docker port → environment `baseUrl` and auth filled in automatically → requests run | Lens or kubectl, plus manual copying | mirrord charges $50 per seat to address this pain ([MetalBear](https://metalbear.com/mirrord/pricing/)) |
| Load test against the mock | Saved flow → open-model load test → local mock first as a baseline, then the real service → run summary committed to git | k6 or Gatling, plus SaaS run comparison | Run comparison is paid elsewhere ([Gatling](https://gatling.io/features/run-trends-and-run-comparison)) |
| One schema → consistent data everywhere | DB schema or OpenAPI → seeded generator → SQL inserts, Redis and Mongo seeds, and mock responses with matching IDs | Mockaroo plus scripts | Mockaroo costs $60–$7,500 a year ([Mockaroo](https://www.mockaroo.com/pricing)) |
| Inline decode | Any JWT, Base64 value or timestamp in a response, header or log line → decoded in place, with expiry warnings | Copy-paste into DevToys or jwt.io | Pasting tokens into websites leaks credentials ([DEV](https://dev.to/rmb_37/stop-pasting-jwts-into-jwtio-12kn)) |
| Explain this failure | Failing request + matching mock or spec + correlated logs + pod events → rule-based checks → local LLM explanation citing those items → fix the user can edit | Manual searching across tools | 48% trust AI when easily validated; 93% value cited sources ([SO 2026](https://survey.stackoverflow.co/2026/ai)) |
| Agents via MCP | Claude Code or Cursor call saved requests, read mocks and query DBs read-only; anything that changes state is confirmed in the app | Nothing comparable | 51% of API developers worry about AI agents making unauthorized or excessive API calls ([Postman SOTA](https://www.postman.com/state-of-api/)) |

The webhook loop deserves first place because it removes three subscriptions and fixes two bugs nobody else addresses. Stripe and Slack signatures expire after five minutes, so replayed events fail, and Twilio signs the exact public URL, so rewriting it breaks validation ([Stripe](https://docs.stripe.com/webhooks); [Twilio](https://www.twilio.com/docs/usage/webhooks/webhooks-security)). Signature verification with an explanation of why it failed, plus re-signing on replay, makes /dev/all better than any surveyed inspector rather than merely cheaper. Keeping unlimited history on local disk turns the vendors' main upgrade lever, retention, into a free default.

Capture → mock → replay is the second anchor because the author's wiremock-ui already implements its hardest middle step. Its "create stub from request" dialog turns numeric IDs and UUIDs in URLs into regexes, offers header and body matcher options, and shows a live preview ([wiremock-ui CreateStubDialog](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/requests/CreateStubDialog.tsx)). Redaction must come before anything is written to git. mitmproxy2swagger's own README warns that including examples can leak tokens, and secret leakage is a recurring concern in the source material.

The cluster-to-environment binding, together with a project dashboard, is the workflow most likely to surprise users, because no source describes it shipping anywhere. kftray already shares port-forward configs through GitHub ([kftray](https://github.com/hcavarsan/kftray)). Writing those forwards into a per-project file that also fills in HTTP base URLs is a small step with a large effect.

The AI workflows follow one rule taken from K8sGPT and from the "verify first" survey data: the model explains facts that code has already extracted and checked, and it never acts on untrusted content without a human in between.

## Tauri and local models fit macOS and Windows; Firebase fits only half the job

### Tauri 2.12 is the right shell for macOS and Windows, with Linux second-tier

Tauri 2.12, announced September 26, 2026, is "the biggest update so far in the 2.x releases". It requires Rust 1.90 and comes with official plugins for the file system, shell, updater, store, SQL, Stronghold, deep links, single instance, window state and more ([Tauri 2.12](https://v2.tauri.app/blog/tauri-2.12/); [plugin index](https://v2.tauri.app/plugin/)). Yaak proves the stack for this exact category of app. One developer who shipped the same app in both frameworks reported a bundle shrinking from 138 MB to 14 MB and idle RAM falling from about 210 MB to 65 MB, though that is a single anecdote ([alanregaya.dev](https://alanregaya.dev/blog/tauri-vs-electron-lessons)).

Two weaknesses shape the design.

**Linux rendering.** The two required platforms are the safe ones. Windows uses the evergreen, Chromium-based WebView2, and macOS uses WKWebView, which updates with the OS ([Tauri webview versions](https://v2.tauri.app/reference/webview-versions/)). Linux uses whatever WebKitGTK version the distribution ships, and on NVIDIA setups its DMABUF renderer often produces blank windows ([Tauri Linux graphics](https://v2.tauri.app/develop/debug/linux-graphics/)). The Chromium runtime (CEF) exists only in the Tauri 3 alpha and makes bundles "hundreds of megabytes larger" ([Tauri v3 CEF](https://v3.tauri.app/develop/cef/)). Linux should therefore launch as a second-tier platform with documented workarounds, as Yaak does ([Yaak Linux docs](https://yaak.app/docs/getting-started/linux-graphics-issues)).

**Throughput between Rust and the UI.** Tauri events carry JSON strings and are "not designed for low latency or high throughput". Tauri's docs recommend Channels and raw `ipc::Response` ArrayBuffers for streaming data instead ([Tauri events](https://v2.tauri.app/develop/calling-frontend/); [Tauri commands](https://v2.tauri.app/develop/calling-rust/)). One third-party benchmark measured a 64 KB binary round trip at about 0.2 ms, against 2.3 ms for a standard invoke ([tauri-conduit](https://github.com/userFRM/tauri-conduit)). Load-test metrics, logs and captured traffic should be aggregated in Rust and pushed to the UI as compact binary frames 4–10 times a second, with request and response bodies fetched on demand into virtualized views.

The architecture should follow Yaak and GitButler, which both put their logic in a shared Rust core used by a thin Tauri shell and by a CLI. Yaak has 36 workspace crates, grouped into core, Tauri, CLI, server and proxy ([Yaak Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/Cargo.toml)). GitButler's `but` CLI runs on "the same Rust backend engine" as its desktop app ([GitButler](https://github.com/gitbutlerapp/gitbutler)). Bruno shows what goes wrong otherwise: its CLI lagged behind its own switch to YAML ([bru-converter](https://cdn.jsdelivr.net/npm/bru-converter@0.1.0/README.md)). The GUI and CLI should use the same parser and the same test fixtures from day one. Yaak also uses `ts-rs` to generate TypeScript types from its Rust models, which keeps the frontend and core from drifting apart.

Distribution needs its own budget line. Tauri updater signatures cannot be disabled, and losing the signing key leaves existing users unable to update. On Windows, EV certificates no longer grant instant SmartScreen reputation ([Tauri updater](https://v2.tauri.app/plugin/updater/); [Windows signing](https://v2.tauri.app/distribute/sign/windows/)). On macOS, distributing outside the App Store requires a paid Apple Developer account, a Developer ID Application certificate and notarization ([Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/)). Both requirements extend to every bundled sidecar, which matters for the local-model runtime described below.

### Files are the source of truth, and the format is designed to avoid merge conflicts

The recommended format is YAML with one file per entity: request, mock route, environment, load scenario, tunnel config or saved query. A root `devall.yaml` holds the schema version, each feature gets its own directory, and a gitignored `.devall/` directory holds local state. This mirrors Bruno's OpenCollection layout and its reasons for choosing YAML ([Bruno docs](https://docs.usebruno.com/opencollection-yaml/overview)).

Three choices avoid the merge problems competitors ran into:

1. **Store ordering in the parent folder's file**, not as a `seq` number inside every request file.
2. **Never write volatile data into tracked files.** Timestamps, the last response and open tabs belong elsewhere; this kind of data is what causes Insomnia's spurious diffs ([Kong](https://developer.konghq.com/insomnia/git-sync/)).
3. **Name files with readable slugs and keep a stable ID inside each file**, rather than Yaak's opaque `yaak.<id>.yaml` names ([Yaak docs](https://yaak.app/docs/using-yaak/local-directory-git-sync)).

Beyond that:

- Generate a JSON Schema for each entity type from the Rust models, as Yaak does with `schemars`, so editors can validate the files outside the app.
- Run format migrations only with the user's consent.
- Serialize mocks so that WireMock mapping JSON round-trips unchanged.

The files are the canonical record. SQLite holds history, blobs, indexes and UI state as a cache that can be rebuilt from the files. Both Yaak and GitButler use rusqlite with a bundled SQLite build ([GitButler Cargo.toml](https://raw.githubusercontent.com/gitbutlerapp/gitbutler/master/Cargo.toml)). One item is unresolved: Yaak still pins `serde_yaml 0.9.34` ([yaak-git Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-git/Cargo.toml)), and this research did not check how well maintained the Rust YAML crates are, so that choice needs verification.

For git itself, gitoxide is pure Rust and complete for status, diff and log, but **push, rebase and reset are missing** and its commit support does not run hooks ([gitoxide README](https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/README.md)). Yaak instead bundles libgit2 and OpenSSL through the git2 crate ([yaak-git Cargo.toml](https://raw.githubusercontent.com/mountain-loop/yaak/main/crates/yaak-git/Cargo.toml)). GitButler uses both and is migrating to gix ([GitButler Cargo.toml](https://raw.githubusercontent.com/gitbutlerapp/gitbutler/master/Cargo.toml)).

A hybrid fits /dev/all best: gix for reading, and the user's own `git` for commit, push and pull, so credential helpers, SSH agents and commit signing keep working. That keeps the in-app git panel small, which suits users who already have their own git workflow.

### Secrets stay out of the repo, and provider integrations stay free

The author's rule that secrets live only on the local machine matches the strongest pattern in the field. Tracked files hold references such as `{{secret.STRIPE_KEY}}`, and the values are filled in at runtime.

Two competitors show the range of approaches:

- **Bruno** stores secret values locally, protected by OS-level encryption with an AES-256 fallback ([Bruno secrets](https://docs.usebruno.com/secrets-management/overview)).
- **Yaak** goes further with optional workspace encryption (XChaCha20-Poly1305) keyed from the OS keychain, so the encrypted values can be committed and shared ([Yaak security](https://yaak.app/docs/getting-started/security-and-privacy)). It also excludes environments from git sync by default ([Yaak docs](https://yaak.app/docs/using-yaak/local-directory-git-sync)).

The Rust keyring ecosystem has split. `keyring-core` 1.0 is now the library API, with a separate crate for each platform's store, and `keyring` 4.x is only a sample crate ([keyring 4.0](https://docs.rs/crate/keyring/4.0.0); [keyring-core](https://docs.rs/keyring-core)).

For /dev/all, the secrets design should:

- store values in the OS keychain through `keyring-core`;
- read gitignored `.env` files;
- offer 1Password, Bitwarden, Vault and the cloud secret managers **for free**, since Postman, Insomnia, Bruno and Thunder Client all paywall these integrations;
- scan for high-entropy strings before writing any tracked file;
- redact secrets from history, exports, generated code and AI prompts, as Hurl does in its reports ([Hurl releases](https://github.com/Orange-OpenSource/hurl/releases));
- give CI a non-interactive path, such as `DEVALL_SECRET_<NAME>` environment variables, because CI machines have no keychain.

gitleaks is now "feature complete" and receives only security patches ([gitleaks](https://github.com/gitleaks/gitleaks)), so its rule format is a useful reference rather than a dependency. Whether to offer Yaak-style committed encrypted values as an opt-in is an open decision: it bends the stated rule in exchange for easier team sharing.

The scratch pad should be an ordinary project stored in the app data directory, in the same file format, so moving an item into a real project is a file move rather than a conversion. That avoids repeating Postman's retirement of its Scratch Pad, which left users with missing collections ([Postman help](https://learning.postman.com/help/resolve-issues/postman-app/i-lost-my-requests-after-migrating-from-the-scratch-pad.md)).

### Scripts run in QuickJS; plugins run as WASM

Pre- and post-request scripts are both a requirement and a security trap. Bruno shipped a security release in September 2026 that restricted which module paths its QuickJS sandbox can load, and Insomnia added an opt-in QuickJS sandbox the same month ([Bruno releases](https://github.com/usebruno/bruno/releases); [Insomnia releases](https://github.com/Kong/insomnia/releases)).

The recommended approach is to embed rquickjs, a binding to QuickJS-NG whose engine code is about 210 KiB, with a Postman-compatible `pm.*` API layer. That gives millisecond startup, a fresh JavaScript context per request, and identical behavior in the GUI and the CLI ([rquickjs](https://docs.rs/crate/rquickjs/0.12.1)). Scalar shows that a `pm`-compatible API is a credible way to win over Postman users ([Scalar](https://scalar.com/products/api-client)).

Extensions such as importers, auth providers, secret providers and template functions should run as WASM components on wasmtime, with versioned WIT interfaces, as Zed does. That sandboxes each extension and limits what it can reach ([Zed](https://www.plushcap.com/content/zed/blog/life-of-a-zed-extension-rust-wit-wasm)). Yaak's plugins, by contrast, run in Node with full system access and are still labeled experimental ([Yaak plugins](https://yaak.app/docs/plugin-development/plugins-quick-start)).

### Local AI: connect to the user's runtime first, offer a sidecar second, use the cloud only with consent

The lowest-risk design has three layers:

1. **Always** support any OpenAI-compatible endpoint, and auto-discover Ollama, LM Studio, Jan and llama-server on both macOS and Windows.
2. **Offer a managed, signed llama.cpp `llama-server` sidecar**, built separately for each platform, for one-click local use.
3. **Run small fixed-purpose models**, for embeddings and PII detection, inside the app process via `ort`: CPU by default on both platforms, with CoreML optional on macOS and DirectML optional on Windows.

llama.cpp is MIT-licensed and supports the widest range of GPUs ([llama.cpp](https://github.com/ggml-org/llama.cpp)). Running it as a sidecar provides chat templates, tool calling and JSON-schema output "with zero native build work", and a GPU driver crash takes down the sidecar rather than the app ([Liquid AI](https://docs.liquid.ai/deployment/on-device/llama-cpp/desktop)). Jan, a Tauri app with bundled llama.cpp engines, is a working precedent ([Jan](https://github.com/janhq/jan)).

Some options to avoid or limit:

- **CrabNebula's `tauri-plugin-llm`:** avoid it, because it is under the PolyForm Noncommercial license ([CrabNebula](https://docs.crabnebula.dev/plugins/llm/)).
- **mistral.rs:** treat it as an optional pure-Rust engine only, because it lacks Vulkan support ([mistral.rs](https://github.com/EricLBuehler/mistral.rs)).
- **Remote providers:** `genai` 0.7 covers 27+ providers and supports structured output ([genai](https://github.com/jeremychone/rust-genai)).
- **Ollama:** its OpenAI compatibility lacks `tool_choice` and image URLs, so features must check what the configured model actually supports ([Ollama](https://docs.ollama.com/api/openai-compatibility)).

Small models need narrowly constrained tasks. Insomnia's own docs warn that models under 10B parameters are less accurate ([Insomnia](https://developer.konghq.com/insomnia/ai-in-insomnia/)). llama-server supports only part of JSON Schema and **silently ignores keywords it does not support**, so every structured output must be validated in Rust ([Liquid AI structured output](https://docs.liquid.ai/deployment/on-device/llama-cpp/structured-output)). The right pattern is K8sGPT's: code first extracts the facts (schema inference, diffs, statistics, rule-based analyzers), then a small model names, explains and drafts, and the result is validated before the user accepts it.

The MCP server needs the same discipline. The 2026-07-28 MCP spec made the protocol stateless, which is a breaking change ([MCP changelog](https://modelcontextprotocol.io/specification/2026-07-28/changelog)). In 2025, MCP SDK vulnerabilities let malicious websites call tools on localhost HTTP servers through DNS rebinding ([NVD](https://nvd.nist.gov/vuln/detail/CVE-2025-66416)).

The recommended setup:

- stdio access through a `devall mcp` command;
- an optional HTTP endpoint bound to 127.0.0.1, with Origin checks and a bearer token;
- read-only tools by default;
- confirmation in the app for anything that writes to a database, applies changes to a cluster, opens a public tunnel or starts a heavy load test ([MCP security](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices)).

The same rule applies to AI inside the app: a model that has read untrusted data must not trigger side effects without a human approving them. Every HTTP response, webhook payload and log line could carry a prompt injection. For the same reason, the managed llama-server sidecar should listen only on 127.0.0.1, on a random port, with an API key.

### The sidecar ships per platform: Metal on macOS, CPU and Vulkan on Windows

macOS and Windows are both first-class targets, and llama.cpp publishes prebuilt `llama-server` binaries for each. The current build, b11796 (October 21, 2026), includes these assets ([GitHub API: llama.cpp releases](https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=1); [llama.cpp releases](https://github.com/ggml-org/llama.cpp/releases)):

| Platform | Build | Download size | Acceleration |
|---|---|---|---|
| macOS, Apple Silicon | `macos-arm64` | 16 MB | Metal |
| macOS, Intel | `macos-x64` | 12 MB | Treat as CPU-only; Metal on Intel Macs was not verified |
| Windows x64 | `win-cpu-x64` | 24 MB | CPU |
| Windows x64 | `win-vulkan-x64` | 37 MB | NVIDIA, AMD and Intel GPUs |
| Windows x64 | `win-cuda-13.4-x64` + CUDA runtime | 160 MB + 424 MB | NVIDIA (the CUDA 12.4 build is 272 MB + 391 MB) |
| Windows ARM64 | `win-cpu-arm64` / `win-vulkan-arm64` | 13 MB / 25 MB | CPU / Vulkan |
| Windows ARM64 | `win-opencl-adreno-arm64` | 14 MB | Snapdragon Adreno GPU |

The same release also has SYCL, ROCm and OpenVINO builds for Windows x64, and a full set of Linux builds.

That sets the packaging plan:

- **macOS:** bundle the Metal arm64 build inside the signed app bundle, plus the 12 MB x64 build for Intel Macs. Treat Intel Macs as CPU-only, which matches Ollama's own "x86 (CPU only)" support ([Ollama macOS](https://docs.ollama.com/macos)).
- **Windows x64:** bundle the CPU and Vulkan builds, about 61 MB together, which covers NVIDIA, AMD and Intel GPUs. CUDA with its runtime adds **roughly 0.6 GB**, so offer it as an on-demand download only after detecting an NVIDIA GPU with a recent driver. No independent comparison of Vulkan and CUDA speed for the recommended models was found, so present CUDA as an optional speed-up, not a requirement.
- **Windows on ARM (Snapdragon):** bundle the ARM64 CPU and Vulkan builds, with the Adreno OpenCL build as an optional download.

Third-party runtimes cover the two platforms unevenly, which is one more reason /dev/all needs its own sidecar:

- **Ollama** runs on both. On macOS it needs version 14 or later, accelerates on Apple Silicon and is CPU-only on Intel Macs. On Windows it needs Windows 10 22H2 or later and supports NVIDIA (driver 551.61+) and AMD through ROCm v7 or Vulkan. Its Windows docs mention neither ARM64 nor Intel GPUs ([Ollama macOS](https://docs.ollama.com/macos); [Ollama Windows](https://docs.ollama.com/windows)). Since v0.40.0 (September 25, 2026), Ollama automatically runs supported architectures on Apple's MLX runtime, including Gemma 4, Qwen3.5, Qwen3.6, Qwen3.8 and EmbeddingGemma-2. Mac users who connect /dev/all to Ollama therefore get MLX speed without /dev/all shipping its own MLX engine ([Ollama releases](https://github.com/ollama/ollama/releases)). Ollama also publishes a standalone Windows zip meant for embedding in other apps, but connecting to an installed Ollama is simpler than bundling one.
- **LM Studio** supports Windows x64 (AVX2 required) and ARM64, but on macOS **only Apple Silicon**: "Intel-based Macs are currently not supported" ([LM Studio requirements](https://lmstudio.ai/docs/app/system-requirements)). On an Intel Mac, /dev/all's own sidecar or Ollama is the only local option.

For the in-process layer, `ort` uses different accelerators on each platform. It offers CoreML on macOS, and DirectML or CUDA on Windows ([ort execution providers](https://ort.pyke.io/perf/execution-providers)). DirectML is now in "sustained engineering": it is still supported, but Microsoft has moved new feature development to Windows ML. DirectML sessions must also disable memory-pattern optimisation and parallel execution, and only one thread may call `Run` at a time ([ONNX Runtime DirectML](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)). Because a 270M–600M embedding model runs acceptably on CPU, the default should be CPU on both platforms. CoreML can be an option on Macs and DirectML an opt-in on Windows until ort's Windows ML support has been checked.

Distribution is the hardest platform work. Windows antivirus engines keep flagging llama.cpp's own binaries. Six GitHub issues between July and September 2026 report Microsoft Defender detecting builds as `Trojan:Win32/Wacatac` and Bitdefender quarantining `llama-server-impl.dll` ([llama.cpp issues](https://github.com/ggml-org/llama.cpp/issues?q=antivirus%20OR%20%22false%20positive%22%20OR%20defender); [#26739](https://github.com/ggml-org/llama.cpp/issues/26739); [#28224](https://github.com/ggml-org/llama.cpp/issues/28224)). To manage this, /dev/all should:

- re-sign every sidecar executable and DLL with its own Authenticode certificate;
- submit each release to Microsoft's false-positive portal;
- download GPU variants from versioned URLs and verify their checksums;
- detect a quarantined sidecar after a failed launch and fall back to "connect to Ollama or LM Studio", with a clear message to the user.

On macOS, every Mach-O binary inside the bundle has to be signed with the hardened runtime and notarized under the Developer ID certificate. Tauri's signing page does not say whether `externalBin` sidecars are signed automatically ([Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/)), so check this in the first build. Gatekeeper quarantines binaries downloaded after installation unless /dev/all signs and notarizes them itself. It is simpler to ship the Metal sidecar inside the bundle and update it with app updates than to fetch it at runtime. Signing is also a reason to keep the number of sidecar variants small.

Linux remains secondary. llama.cpp publishes Linux CPU, Vulkan, CUDA, ROCm, SYCL and OpenVINO builds, and LM Studio ships a Linux AppImage, so the model layer itself works there. Linux stays second-tier because of the WebKitGTK rendering problems described earlier, not because of the AI runtime.

/dev/all also needs a version policy:

- Pin one llama.cpp build per /dev/all release, at least b11796 for the October 2026 lineup, and record a minimum build for each curated model.
- Read Ollama's `/api/version` and warn below 0.40.0 for Qwen3.8, Gemma 4 and EmbeddingGemma-2 ([Ollama releases](https://github.com/ollama/ollama/releases)).
- Run a CI smoke test for each curated model that loads it, requests JSON-schema output and makes a tool call. The minimum llama.cpp build for each model architecture could not be verified, so this test is the practical safeguard.

### Qwen is on 3.8, but only one Qwen3.8 model fits a laptop

Qwen published Qwen3.8 open weights in August 2026, but the lineup is narrower than the version number suggests:

- **Qwen3.8-27B** is a dense 27B model under **Apache-2.0**. It accepts image and video input, has 262K native context and thinks by default ([Qwen3.8-27B](https://huggingface.co/Qwen/Qwen3.8-27B)).
- **Qwen3.8-Flash-Next** is a mixture-of-experts model with 125B total and 6B active parameters, under a "qwen-community-1.0" licence ([Flash-Next](https://huggingface.co/Qwen/Qwen3.8-Flash-Next)). It is not laptop-class.
- **Qwen3.8-2.4T-A95B** uses a "qwen3.8-max" licence ([2.4T-A95B](https://huggingface.co/Qwen/Qwen3.8-2.4T-A95B)) and is not relevant for local use.

There is **no official Qwen3.8 model of 9B or smaller, and no Qwen3.8-Coder**. Qwen3.7 has no open weights at all: it exists only as API models that appear in comparison tables ([HF API: Qwen models](https://huggingface.co/api/models?author=Qwen&sort=createdAt&direction=-1&limit=60)). The newest official small Qwen models are therefore still **Qwen3.5 at 0.8B, 2B, 4B and 9B** (February 2026). The newest Qwen model with documented fill-in-the-middle (FIM) support, which inline autocomplete needs, is Qwen3-Coder ([Qwen3-Coder](https://github.com/QwenLM/Qwen3-Coder)). The "Qwen3.8-2B/4B/9B-Distill" GGUF repositories from empero-ai are **third-party distillations, not Qwen releases**, and should not be offered as small Qwen3.8 defaults ([HF API: search Qwen3.8](https://huggingface.co/api/models?search=Qwen3.8&sort=downloads&direction=-1&limit=60)).

Qwen3.8-27B is widely packaged. It has GGUF builds from ggml-org, unsloth, lmstudio-community and bartowski, MLX builds, and an 18 GB Ollama tag `qwen3.8:27b` labelled with vision, tools and thinking ([Ollama: qwen3.8](https://ollama.com/library/qwen3.8)). Its model card documents neither its tool-calling format nor FIM, so tool calling needs a smoke test and FIM should be assumed absent.

This report recommends no Qwen2.5 model, because the family is too old. It matters only as a trap. llama.cpp's small FIM presets (`--fim-qwen-1.5b-default`, `--fim-qwen-3b-default`, `--fim-qwen-7b-default`, and the `-7b-spec` and `-14b-spec` variants) all still load Qwen 2.5 Coder, and llama.vscode uses the same presets for every tier below 64 GB ([llama.cpp server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md); [llama.vscode](https://github.com/ggml-org/llama.vscode)). /dev/all must choose its own FIM models and never ship those presets. The one acceptable preset is `--fim-qwen-30b-default`, which, despite its name, loads Qwen3-Coder-30B-A3B.

Other families have released local-friendly models since April 2026 that fill the gaps Qwen leaves:

- **Gemma 4 12B** (dense, May 2026, Apache-2.0) has 256K context and native structured tool use ([Gemma 4 12B](https://huggingface.co/google/gemma-4-12B-it)).
- **IBM Granite 4.2** (3B, 8B and 30B dense, August 2026, Apache-2.0) ships official GGUF and MLX builds and documents tool calling ([Granite 4.2 8B](https://huggingface.co/ibm-granite/granite-4.2-8b)).
- **JetBrains Mellum2** is a code mixture-of-experts model with 12B total and 2.5B active parameters, under Apache-2.0, but its card does not mention FIM ([Mellum2](https://huggingface.co/JetBrains/Mellum2-12B-A2.5B-Base)).

Meta's newest open models are still Llama 4 from April 2025, and Microsoft's small Phi text models are still the Phi-4-mini family ([HF API: meta-llama](https://huggingface.co/api/models?author=meta-llama&sort=createdAt&direction=-1&limit=25); [HF API: Phi](https://huggingface.co/api/models?author=microsoft&search=phi&sort=createdAt&direction=-1&limit=30)).

### A hardware-tiered model lineup for macOS and Windows

The curated defaults below cover general and tool-calling work, code and inline autocomplete, and embeddings for each hardware tier. Sizes are approximate 4-bit file sizes, the context cache needs memory on top, and the list should be re-checked every quarter.

| Hardware tier | General, tool calling, structured JSON | Code and inline autocomplete (FIM) | Embeddings |
|---|---|---|---|
| **~8 GB RAM**: 8 GB M1/M2 Macs, Intel Macs, 8 GB Windows laptops on CPU or integrated GPU | Qwen3.5-4B (≈3.4 GB) or Gemma 4 E4B (≈4.5 GB) | Mellum-4b for FIM (a 2025 model; no 2026 small FIM model was found). Code chat uses the general model. Run one model at a time | EmbeddingGemma-2, 270M text-only, on CPU |
| **16 GB**: base M-series Macs, 16 GB Windows laptops, 6–8 GB VRAM | Gemma 4 12B (≈6.7 GB) by default; Qwen3.5-9B (≈6.6 GB) or Granite 4.2 8B (5.35 GB) as alternatives | Mellum-4b for FIM; Qwen3.5-9B or Mellum2 Instruct (8.07 GB) for code chat | EmbeddingGemma-2 |
| **24–32 GB+ unified memory**: M-series Pro, Max and Ultra | **Qwen3.8-27B**: UD-Q4_K_M (16.5 GB) on 32 GB+, UD-Q3_K_XL (13.2 GB) on 24 GB. Faster mixture-of-experts alternatives: Qwen3.6-35B-A3B or Gemma 4 26B-A4B (14.4 GB) | **Qwen3-Coder-30B-A3B**: Q4_K_M (18.6 GB) or IQ4_XS (16.4 GB). On 24 GB Macs, use Q3_K_M (14.7 GB) or Mellum-4b | EmbeddingGemma-2 or Qwen3-Embedding-0.6B; Qwen3-Reranker-0.6B for reranking |
| **64 GB+ unified memory** | As above | Qwen3-Coder-Next, 80B total with 3B active (≈38 GB) | As above |
| **Windows, NVIDIA 8 GB VRAM** (CUDA sidecar or Ollama) | Qwen3.5-9B, Gemma 4 12B or Granite 4.2 8B, fully on the GPU | Mellum-4b | EmbeddingGemma-2 on CPU |
| **Windows, NVIDIA 12–16 GB VRAM** | Gemma 4 26B-A4B (14.4 GB), or Qwen3.8-27B at UD-IQ3_XXS (10.9 GB) or UD-Q3_K_XL (13.2 GB) | Qwen3-Coder-30B-A3B Q3_K_M (14.7 GB, tight on 16 GB) or Mellum-4b | EmbeddingGemma-2 |
| **Windows, NVIDIA 24 GB VRAM** (RTX 3090/4090/5090 class) | Qwen3.8-27B UD-Q4_K_M (16.5 GB), leaving room for context; Qwen3.6-35B-A3B for speed. Nemotron 3.5 Bolt 74B-A9B-24GB is experimental | Qwen3-Coder-30B-A3B Q4_K_M (18.6 GB) | EmbeddingGemma-2 |
| **Windows, AMD or Intel GPU** | Same VRAM tiers, using the Vulkan build; speed was not measured | Same | Same |

Sources: [Qwen3.5 small models](https://artificialanalysis.ai/articles/qwen3-5-small-models); [InsiderLLM](https://insiderllm.com/guides/best-local-coding-models-2026/); [Gemma 4 sizes](https://ai.google.dev/gemma/docs/core); [Gemma 4 12B](https://huggingface.co/google/gemma-4-12B-it); [Granite 4.2 8B GGUF](https://huggingface.co/api/models/ibm-granite/granite-4.2-8b-GGUF/tree/main); [Qwen3.8-27B GGUF](https://huggingface.co/api/models/unsloth/Qwen3.8-27B-GGUF/tree/main); [Qwen3-Coder-30B-A3B GGUF](https://huggingface.co/api/models/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/tree/main); [Mellum2 Instruct GGUF](https://huggingface.co/api/models/JetBrains/Mellum2-12B-A2.5B-Instruct-GGUF-Q4_K_M/tree/main); [Nemotron 3.5 Bolt](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB); [local_models_update_oct2026.md](../research_notes/Developer%20toolkit%20competitive%20research/local_models_update_oct2026.md).

**Autocomplete is the weakest tier.** The Qwen3-Coder README says "FIM is supported in every version of Qwen3-Coder" and documents the prompt format ([Qwen3-Coder](https://github.com/QwenLM/Qwen3-Coder)). Qwen3-Coder-30B-A3B is Apache-2.0 and, with 3B active parameters, has low per-token latency; it can be served through llama-server's `/infill` endpoint. Small machines have no equivalent. The research found **no model released in 2026 at 8B or smaller with documented FIM support**: Qwen3.5, 3.6 and 3.8 are not FIM models, and the cards for Gemma 4 12B, Granite 4.2 8B and LFM2.5-24B-A2B do not mention FIM. The best confirmed small option is **JetBrains Mellum-4b**, a 2025 model under Apache-2.0 with an 8K context, explicit FIM tokens and a stated focus on IDE code completion ([Mellum-4b](https://huggingface.co/JetBrains/Mellum-4b-dpo-all)). Its file size and latency were not measured. When no FIM model is installed, /dev/all can complete from the prefix alone using the general model, clearly labelled as lower quality. Mellum2 and the upstream llama.cpp presets should be re-checked each quarter.

**EmbeddingGemma-2 is the default embedding model.** Google released it in September 2026 under Apache-2.0, without the gated terms of the first version. It loads as a 270M text-only model or, with image and audio support, up to 740M. It produces 768-dimension vectors that can be truncated to 512, 256 or 128 dimensions, accepts 8K tokens of context, and scores 78.68 on MTEB Code v1, up from 68.76 for version 1 ([EmbeddingGemma-2](https://huggingface.co/google/embeddinggemma-2)). Ollama already ships it and runs it on MLX on Apple Silicon. No official ONNX export was verified, so at first /dev/all should run it through Ollama or the llama-server sidecar on both platforms, and move it in-process via `ort` once an export exists. llama-server's `--embd-gemma-default` preset does not say which version it loads, so /dev/all should pin the model file explicitly. The fallback is Qwen3-Embedding-0.6B (Apache-2.0, official GGUF). Each index should store the model ID and vector dimension, so that changing the model triggers a rebuild rather than a silent mismatch.

Four handling rules apply to every tier:

- **Turn thinking off by default.** Qwen3.5 and Qwen3.8 think by default and turn it off with `enable_thinking: false` in the chat template arguments; Gemma 4 and Granite 4.2 have similar switches. LFM2.5-24B-A2B "always thinks", so keep it out of latency-sensitive features.
- **Constrain and validate structured output.** Use JSON-schema or grammar-constrained decoding, and validate the result in Rust.
- **Only Apache-2.0 models as one-click defaults.** Flag models under "qwen-community-1.0", "lfm1.0" or OpenMDW-1.1 for licence review before offering them.
- **Treat benchmark numbers as provisional.** They are vendor-reported on different test harnesses; for example, Granite 4.2 8B reports 52.39 on BFCL v4 while Qwen3.5-9B self-reports 66.1. No tokens-per-second figures were measured on base Macs or on Windows with Vulkan. /dev/all should check the lineup against its own task suite before every release.

### Firebase can capture webhooks but cannot carry a tunnel

Firebase can do half of the cloud job. A 2nd-gen HTTPS function can receive a webhook and write it to Firestore, and a snapshot listener in the desktop app can forward it to localhost. This is Hookdeck-style store-and-forward, and it fits within the free quotas (2M function invocations a month and 20,000 Firestore writes a day), though it requires the pay-as-you-go Blaze plan ([Firebase pricing](https://firebase.google.com/pricing); [Firestore quotas](https://firebase.google.com/docs/firestore/quotas)).

Firebase fails for tunnels. Hosting rewrites cut requests off at **60 seconds**, 2nd-gen functions are capped at 60 minutes, the quotas page documents no WebSocket support, and the Realtime Database allows only 100 simultaneous connections on the free Spark plan ([Firebase Hosting](https://firebase.google.com/docs/hosting/functions); [Functions quotas](https://firebase.google.com/docs/functions/quotas); [Firebase pricing](https://firebase.google.com/pricing)). The lack of WebSocket support is inferred from what the documentation omits; Google does not state it outright.

You could fake a tunnel by having a function wait for the desktop app to write a response document, but that adds latency and bills function time for the whole wait. It is acceptable for capturing webhooks and unusable for browsing a dev site or hot reload. Firestore's 1 MiB document limit also means large payloads would need Cloud Storage ([Firestore quotas](https://firebase.google.com/docs/firestore/quotas)).

| Option | Webhook capture | HTTP/WebSocket tunnel | Raw TCP | Indicative hobby cost | Main caveat |
|---|---|---|---|---|---|
| Firebase (Functions + Firestore) | Yes, store-and-forward | No: 60-second Hosting cap and no documented WebSockets ([Hosting](https://firebase.google.com/docs/hosting/functions)) | No | $0 within free quotas; Blaze plan with a card required | Cold starts against Shopify's 5-second budget; 1 MiB documents |
| Cloud Run | Yes | Works, but every open socket keeps an instance billed as active, and connections must reconnect every 60 minutes ([Cloud Run](https://docs.cloud.google.com/run/docs/triggering/websockets)) | No | Not verified (the pricing page did not render) | Routing between instances needs pub/sub |
| Cloudflare Workers + Durable Objects | Yes, with a SQLite buffer in the Durable Object | Yes, as a WebSocket hub that hibernates when idle ([DO WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)) | No inbound TCP ([Workers TCP](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/)) | About $5/month flat, no egress fees ([Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/)) | Every deploy disconnects all sockets |
| Fly.io machine or VPS | Yes | Yes | Yes | About $4.20/month on Fly ([Fly](https://docs.fly.io/about/pricing)); about €4.5–6.5 at Hetzner, where reported prices conflict ([Hetzner](https://www.hetzner.com/cloud/)) | You operate and patch it yourself |
| Supabase Realtime | Possible via broadcast | Not designed for byte streams | No | $0, but free projects pause after a week idle; Pro is $25/month ([Supabase](https://supabase.com/pricing)) | Pausing rules it out for an always-on relay |

The recommended design has three parts:

1. A Cloudflare Worker handles subdomain routing and abuse checks.
2. A Durable Object per project acts as the WebSocket hub, so hibernation applies when idle. It keeps a short SQLite buffer for webhooks that arrive while the laptop is asleep.
3. A Rust client in the desktop app connects over WSS on port 443, which gets through corporate proxies.

The WebSocket can carry several multiplexed streams, following rathole's model of pluggable transports ([rathole](https://github.com/rapiz1/rathole)). Workers cannot accept inbound raw TCP, so raw TCP tunnels can come later on a Fly.io machine. Letting users bring their own ngrok token through the official `ngrok` Rust crate, or fall back to a spawned `cloudflared`, costs /dev/all nothing to run ([ngrok-rust](https://github.com/ngrok/ngrok-rust)). Firebase is still a reasonable home for accounts and license records if the author prefers that ecosystem. It just stays out of the path that webhook and tunnel traffic takes.

Running a public relay also brings abuse responsibilities. Proofpoint tracked malware campaigns that delivered remote-access trojans through Cloudflare's no-account TryCloudflare tunnels ([Proofpoint](https://www.proofpoint.com/us/blog/threat-insight/threat-actor-abuses-cloudflare-tunnels-deliver-rats)). ngrok shows a warning page before serving HTML to browsers on its free tier ([ngrok](https://ngrok.com/docs/pricing-limits/free-plan-limits)).

The standard mitigations apply:

- Require sign-in for public tunnels. Capture-only webhook URLs can stay open as long as they never serve HTML.
- Show a warning page only for `text/html` responses, never for API clients.
- Use a dedicated tunnel domain registered on the Public Suffix List, as ngrok, Fly and Cloudflare do ([PSL](https://raw.githubusercontent.com/publicsuffix/list/main/public_suffix_list.dat)).
- Keep per-account quotas and a kill switch for each subdomain.

## Carry wiremock-ui over as both the design system and the mock module

wiremock-ui fits /dev/all closely. It is built with Vite 8, React 19, TypeScript 6, TanStack Query 5, Zustand 5, Monaco 0.57 and Lucide icons, styled with plain CSS driven by design tokens, with no router or component library ([package.json](https://github.com/alexmbrown/wiremock-ui/blob/main/package.json)). React with Vite is Tauri's default frontend template, so the code moves over with little change.

The design brief describes the target look as "a precise developer tool, in the family of Linear, Vercel dashboard, Raycast, VS Code, Insomnia/Bruno and Chrome DevTools… calm and compact, with information density first" ([DESIGN_BRIEF.md](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)). Every visual value is a CSS custom property in one `tokens.css` file, switched by `data-theme` and `data-density` attributes. The defaults are **13 px text, 28 px rows and controls, 4/6/10 px corner radii, 120 ms transitions** and a single indigo accent (`#4f46e5` in light mode, `#6d68f5` in dark) ([tokens.css](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/tokens.css)).

Much of the UI can be carried over as-is:

- `tokens.css` and `app.css`, about 920 lines together.
- The UI primitives: menus, modals, confirmation dialogs that require typing the server name for destructive actions, toasts with Undo, segmented controls and resizers ([app.css](https://github.com/alexmbrown/wiremock-ui/blob/main/src/styles/app.css)).
- The app shell:
  - a 44 px top bar with project and environment switchers;
  - a 188 px side navigation with grouped sections and counts;
  - a page header and toolbar;
  - split panes with VS Code-style tabs (preview, dirty and conflict states);
  - a Ctrl+K command palette with `g`-key navigation.

Some pieces carry over in meaning as well as code. The semantic colors apply directly to the HTTP client, webhook inbox and load-test results: each HTTP method has its own color, status classes run from green 2xx to red 5xx, and environment chips mark production in red. The traffic panel, which shows key metrics, stacked columns, a status-mix bar and a keyboard-navigable SVG chart, is a ready-made dashboard for load tests and webhooks ([TrafficPanel.tsx](https://github.com/alexmbrown/wiremock-ui/blob/main/src/features/dashboard/TrafficPanel.tsx)). The navigation's three groups (main, Tools, Configure) map naturally onto /dev/all's modules. The design hand-off workflow is also worth keeping: generated screenshots, a component catalog and a brief written for Claude Design.

About 3,000 lines of framework-free, unit-tested TypeScript in `src/lib/` can be ported verbatim:

- `types.ts`, whose index signatures preserve unknown fields when a stub is saved back;
- `match.ts`, a client-side WireMock matcher that reports which fields matched and which failed;
- `fix.ts`;
- `request-to-stub.ts`;
- `har.ts`;
- `importers.ts`, covering cURL, OpenAPI, Postman, HAR and ZIP;
- `codegen.ts`;
- `templates.ts`, with 20 stub templates and about 36 Handlebars snippets;
- `capabilities.ts`.

Several of these become shared infrastructure for the whole toolkit. The cURL, OpenAPI and Postman importers can feed the HTTP client. `request-to-stub` can feed the webhook inbox and the traffic inspector. `match.ts` is also the clearest specification for a Rust port of WireMock matching, quirks included ([match.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/match.ts)).

/dev/all should keep wiremock-ui's metadata conventions, `metadata["wiremock-gui"].folder` and `metadata.wmui`, so wiremock-ui, holomekc's GUI and /dev/all can all manage the same WireMock server ([stub-utils.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/stub-utils.ts)). The author's `useWrite()` wrapper refuses writes on read-only servers, asks for confirmation on protected ones, shows toasts and refreshes cached data. It should become the safety layer for every connection type, including databases, Kubernetes and Docker, since the author has already designed production-safety modes ([queries.tsx](https://github.com/alexmbrown/wiremock-ui/blob/main/src/lib/queries.tsx)).

Several things need to change:

- **Projects and credentials.** The project and server lists move from browser localStorage into project files. Credentials, which today sit in plain text in localStorage when "remember" is ticked ([projects.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/projects.ts)), move to the OS keychain.
- **Admin calls.** Routing calls to the WireMock admin API through Rust removes the browser workarounds for mixed content, CORS and Local Network Access.
- **File handling.** Browser Blob downloads become Tauri file dialogs.
- **Routing.** The flat `Section` enum router is too simple for a multi-module app ([nav.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/src/store/nav.ts)).
- **Tooling.** Add a linter and component tests, which the repo does not have yet.

Tauri also removes the limits the wiremock-ui README states: the browser app cannot start WireMock, cannot read the `mappings/` folder and gets no push updates ([README.md](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md)). A desktop app can manage a WireMock sidecar or run its own engine, read the root directory directly and stream events.

The port is also a chance to fix the brief's own list of known issues: a flat light theme, faint borders in the dark theme, inconsistent badge sizes and a crowded top bar ([DESIGN_BRIEF.md](https://github.com/alexmbrown/wiremock-ui/blob/main/design/DESIGN_BRIEF.md)). The current tokens only include palettes for HTTP methods and charts, so /dev/all needs new color families to give modules such as K8s, Docker, databases and AI their own identity.

The editor choice is an open question. Monaco is 1.37 MB of wiremock-ui's 15 MB build ([vite.config.ts](https://github.com/alexmbrown/wiremock-ui/blob/main/vite.config.ts)). For dozens of small embedded editors, CodeMirror 6 is lighter: Sourcegraph cut its JavaScript download from 6 MB to 3.4 MB by switching, though that report predates 2025 ([Sourcegraph](https://sourcegraph.com/blog/migrating-monaco-codemirror)). Keeping Monaco for schema-validated JSON and for scripts remains a defensible choice.

Finally, the author removed wiremock-ui's "Test request" page so the app would stay focused on administration ([README.md](https://github.com/alexmbrown/wiremock-ui/blob/main/README.md)). In /dev/all that capability comes back as a link between the HTTP client and the mock module, not as duplicate code.

## Build order: a tiered feature map and the decisions it depends on

The map below orders features so that each tier builds on the shared foundations and connections from earlier tiers, and so that the early tiers replace the most-used subscriptions without adding server costs.

| Tier | What ships | Why at this point | Subscriptions it replaces |
|---|---|---|---|
| **0: Foundations** | Project format spec and JSON Schemas; scratch project; secret references, keychain storage, `.env` support and a secret scanner; Rust core crates with a CLI from day one; SQLite history and blob store; event store with correlation IDs; registry of value detectors and operations; wiremock-ui's shell, tokens and command palette ported | Every cross-tool workflow depends on these, and changing the file format later is the most expensive mistake (Bruno's CLI lag, Insomnia's metadata noise) | None directly; enables everything else |
| **1: The wedge** | HTTP client: REST, GraphQL, WebSocket and SSE; inherited environments; OAuth2/PKCE with a localhost callback; certificates, proxies and cookies; history stored as diffable files; imports from cURL, Postman, OpenAPI, HAR, `.http` and Bruno. WireMock-compatible mock engine with wiremock-ui's editor, near-miss explanations, create-stub-from-request, templates, scenarios and chaos presets. Request → mock → randomize. Core utilities with inline decoding: JWT with JWKS verification, encoders, formatters, semantic JSON diff, regex, timestamps, IDs, cURL ↔ code. Collection runner and CLI with JUnit output. Read-only MCP server. Connection to any OpenAI-compatible model, with automatic discovery of Ollama (v0.40+) and LM Studio on macOS and Windows, and first AI helpers: natural language → regex or JSONPath checked against sample strings, and mock data constrained to the schema | wiremock-ui supplies the mock module and the design; the README's flagship workflow lives here; costs nothing to run | Postman Solo/Team; Mockoon Cloud, WireMock Cloud, Beeceptor; DevUtils |
| **2: The public edge** | Hosted relay. Webhook inbox with a stable per-project URL, offline buffering, signature verification and re-signed replay, and save-as-fixture. HTTP, WebSocket and SSE tunnel, with bring-your-own ngrok or cloudflared as fallbacks. MITM inspector with a local CA, breakpoints, Map Local/Remote, and capture to stub or HAR. Log viewer, first for the project's own event stream, then for files and Docker. gRPC in the HTTP client | The largest subscription savings and the hardest workflows for others to copy; the relay is the natural first paid component | ngrok, webhook.site, Hookdeck's developer tier, Proxyman, HTTP Toolkit Pro |
| **3: Depth and AI** | Load tester: open model by default, HDR histograms with coordinated-omission correction, thresholds that set the exit code, run summaries committed to git and compared, and a baseline run against the local mock. Relational, seeded fake-data generator. Lightweight Postgres, MySQL and SQLite, Redis and Mongo clients, with queries saved as files, read-only by default and local natural language → query checked with EXPLAIN. Docker viewer. K8s port-forward manager, logs and environment binding. A managed, signed llama-server sidecar (Metal on macOS; CPU and Vulkan on Windows, with CUDA on demand), together with the hardware-tiered model list: inline autocomplete with Qwen3-Coder-30B-A3B or Mellum-4b, and local semantic search with EmbeddingGemma-2. Local-AI features: failure diagnosis, traffic → OpenAPI and drift checks | Builds on Tier 1's request format and Tier 2's captured traffic; strong free competitors exist here, so "good enough and connected" beats breadth | k6 Cloud, Gatling Enterprise, TablePlus, Lens Plus, Mockaroo |
| **4: Later** | GCP subset: Pub/Sub emulator UI, Cloud Storage, Logging tail, Secret Manager import, Cloud SQL proxy. Distributed load-test agents. Raw TCP tunnels on Fly. gRPC, GraphQL and MQTT mocks. WASM plugin SDK. Recipes with branching and loops. MCP client and inspector. Image and directory diff. Real-time co-editing | Valuable but niche, or depends on the earlier foundations | Partly: DBeaver Ultimate's cloud explorer, LocalStack-style UIs |
| **Skip** | Container runtime (replacing Docker Desktop or OrbStack); Helm or Compose orchestration (call the existing tools); three-way merge; an embedded `mongosh` | Others already do these well, or they are outside a toolkit's scope | None |

Tier 1 comes first because the author has already built half of it, it removes the most widely held subscriptions, and it needs no servers. The README's own example, turning an HTTP request into a mock endpoint and then randomizing its data ([README](../README.md)), is a Tier 1 workflow. It is also the right first proof that a combined app beats separate tools.

Tier 2 comes before load testing and databases for three reasons. Its workflows (relay plus inbox plus client, and inspector plus mock) are the ones single-purpose competitors find hardest to copy. The relay is the only component with ongoing server costs, which makes it the honest place to charge money. And the inspector produces the captured traffic that Tier 3's traffic-to-spec and load-recording features need.

The data and infrastructure viewers come last despite being on the original list. Strong free tools already cover them: Another Redis Desktop Manager, Freelens, Headlamp, k9s and Lazydocker ([ARDM](https://github.com/qishibo/AnotherRedisDesktopManager); [Freelens](https://github.com/freelensapp/freelens); [k9s](https://github.com/derailed/k9s); [Lazydocker](https://github.com/jesseduffield/lazydocker)). Their value inside /dev/all comes mostly from connecting to the other modules: seeding data, filling in environments and sending logs to the shared viewer.

### Open decisions for the author

| Decision | Options | Where the evidence leans |
|---|---|---|
| Mock engine | Native Rust engine that reads WireMock JSON; managed WireMock sidecar (Java or Docker); both | A native engine for the common matchers with WireMock JSON as the on-disk format, plus "attach to any WireMock" through the admin API wiremock-ui already speaks. A Java sidecar gives perfect fidelity but adds a JRE to the installer |
| Request file format | Own YAML schema; adopt OpenCollection | OpenCollection is an open spec with a JSON Schema and is Bruno's default ([spec](https://spec.opencollection.com/)). Adopting it buys compatibility with Bruno but gives up control of the schema for non-HTTP entities |
| Relay host and relay license | Firebase, Cloudflare Durable Objects or Fly/VPS; if open-sourced, AGPL or MIT | Cloudflare Durable Objects for HTTP and WebSocket, Fly later for raw TCP. Hacker News commenters argued for AGPL to stop others rebranding a hosted copy ([HN](https://news.ycombinator.com/item?id=47425918)) |
| Business model | Yaak-style commercial license with a lifetime fallback; paid features on an open core; relay subscription | A commercial license plus a relay priced at or below ngrok Hobbyist's $10/month ([Yaak](https://yaak.app/pricing); [ngrok](https://ngrok.com/pricing)). Never charge for storage location, history or secrets |
| Secret sharing | References only; optional committed encrypted values | References only by default, per the README's rule. Committed encrypted values only as an explicit opt-in if teams ask for it |
| Scripting | No scripts (template functions, like Yaak); rquickjs with a `pm.*` compatibility layer; a Node escape hatch | rquickjs with the compatibility layer, to ease migration from Postman. Node only as an opt-in |
| Editor stack | Monaco everywhere; CodeMirror 6 for small editors and Monaco for scripts | Measure memory use with many editor instances open before committing |
| Git integration | gix for reads and system git for writes; bundled git2 | The hybrid, with a minimal git panel |
| AI defaults | Local-only by default or remote allowed per project; bundled sidecar or connect-only | Local by default, with a preview of exactly what will be sent before any remote call. Bundle the small sidecar builds (Metal; CPU and Vulkan) and download CUDA and model weights on demand |
| Shipping the llama.cpp sidecar | Bundle and re-sign it; connect-only to Ollama or LM Studio | Bundle and re-sign it with Authenticode and Developer ID, notarize it on macOS, and fall back to connect-only when antivirus quarantines it. Intel Macs need the sidecar or Ollama, because LM Studio excludes them ([LM Studio](https://lmstudio.ai/docs/app/system-requirements)) |
| Autocomplete on small machines | Mellum-4b; no FIM below 24 GB; prefix-only completion from the general model | Offer Mellum-4b as an opt-in download, use prefix-only completion as the fallback, and re-check each quarter for a 2026 small model with FIM. Never use llama.cpp's Qwen2.5-based FIM presets |
| Licences for one-click models | Apache-2.0 only; also allow "qwen-community-1.0", "lfm1.0" and OpenMDW-1.1 | Apache-2.0 only for one-click defaults (Qwen3.5, Qwen3.8-27B, Gemma 4, Granite 4.2, Qwen3-Coder, Mellum, EmbeddingGemma-2). Show other licences only after the user reviews them |
| Platform priority | macOS and Windows first-class with Linux second-tier; equal support for all three; wait for Tauri's Chromium runtime (CEF) | macOS and Windows first-class. Linux second-tier with documented workarounds, because of WebKitGTK, not the AI runtime, which has Linux builds |
| Write access to infrastructure | Read-only by default; full control | Read-only by default, with protected environments and type-to-confirm, generalized from wiremock-ui |
| Accounts | Fully account-free; sign-in only for the public relay | Keep the app account-free and require sign-in only for public tunnels, to manage abuse |

## Conclusion

The research shifts the main design question from which tools to include to which foundations to share. Every high-value workflow is a path through the same four foundations: one request/response format, one schema-and-data engine, one event store with correlation IDs, and one registry of value detectors. Competitors built around a single tool's data model cannot add these cheaply, which is why none of them offers the full chain. This also makes the business model straightforward. Everything local costs almost nothing to run, so only the relay costs money, and charging for the one feature that genuinely needs a server avoids paywalling basic features, the pattern behind every backlash in this market.

The fastest credible path is already half built. wiremock-ui is effectively Tier 1's mock module plus the whole design system, and adding the HTTP client brings back the "Test request" capability the author removed, this time as a link between modules. The biggest risks are the commitments made early, not the feature list: the file format, the secrets rule and the license. Get those right and the module list can grow without breaking user trust.

The evidence has limits. Developer sentiment came mostly from Hacker News and blogs rather than Reddit, several prices come from third parties, and no source measures whether people will pay for the cross-tool workflows themselves. A cheap test of that demand, such as a waitlist built around the webhook loop, should come before the relay that Tier 2 depends on. The local-model guidance will date fastest. Qwen moved from 3.5 to 3.8 within six months, the small models still lack a 2026 option with documented FIM, and no speed was measured on base Macs or with Vulkan on Windows. The curated lineup and the per-platform sidecar builds should be treated as a list to re-validate every quarter, not a fixed choice.

## Sources

Citations appear inline throughout. The report synthesizes these research notes in `research_notes/Developer toolkit competitive research/`:

- [api_clients.md](../research_notes/Developer%20toolkit%20competitive%20research/api_clients.md)
- [mocking_and_interception.md](../research_notes/Developer%20toolkit%20competitive%20research/mocking_and_interception.md)
- [webhooks_and_tunnels.md](../research_notes/Developer%20toolkit%20competitive%20research/webhooks_and_tunnels.md)
- [load_testing.md](../research_notes/Developer%20toolkit%20competitive%20research/load_testing.md)
- [utility_toolboxes.md](../research_notes/Developer%20toolkit%20competitive%20research/utility_toolboxes.md)
- [data_and_infra_clients.md](../research_notes/Developer%20toolkit%20competitive%20research/data_and_infra_clients.md)
- [ai_features_and_local_llm.md](../research_notes/Developer%20toolkit%20competitive%20research/ai_features_and_local_llm.md)
- [market_gaps_and_synergy.md](../research_notes/Developer%20toolkit%20competitive%20research/market_gaps_and_synergy.md)
- [architecture_and_tech.md](../research_notes/Developer%20toolkit%20competitive%20research/architecture_and_tech.md)
- [wiremock_ui_analysis.md](../research_notes/Developer%20toolkit%20competitive%20research/wiremock_ui_analysis.md)
- [local_models_update_oct2026.md](../research_notes/Developer%20toolkit%20competitive%20research/local_models_update_oct2026.md), dated October 21, 2026

Where the October 2026 local-models update disagrees with the earlier AI notes, the update wins. It covers the model lineup, autocomplete models, embeddings and macOS and Windows runtime support.

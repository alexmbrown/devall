# Market Gaps, Subscription Fatigue & Cross-Tool Synergy for an All-in-One Local-First Dev Toolkit (/dev/all)

Research date: 2026-10-21. Unless noted otherwise, every price below was read from the vendor's own pricing page on 2026-10-21. Anything dated before 2025 is flagged "(pre-2025, possibly outdated)". Method note: the shared web-search budget ran out partway through, so Hacker News and Reddit threads could not be searched directly. Developer-sentiment evidence comes from surveys, vendor and indie-founder blogs, and search-result snippets. See the Gaps sections.

---

## 1. Existing consolidated / all-in-one dev platforms: trajectory, what worked, what backfired

### Takeaway
Through 2025-2026 the market split two ways. The incumbent "API platform" (Postman) consolidated upward toward enterprise and AI, with metered AI credits and the March 2026 removal of free team collaboration. Meanwhile, file-based, git-native, no-login tools (Bruno, Yaak, Hoppscotch, DevToys) won goodwill and grew. The strongest signal is that Postman itself shipped "Local by default" Native Git storage, one YAML file per request, in 2026. That confirms "files in git" is now table stakes rather than a differentiator. Nobody has combined API client, mocks, tunnel/webhooks, load testing, DB, k8s and utilities around one project folder. Each incumbent covers 1-3 of these categories.

### Cited Findings

**Postman (incumbent API platform)**
- Before March 2026 Postman sold Free, Basic, Professional and Enterprise plans. Since March 2026 it sells Free, Solo, Team and Enterprise, and Team replaces Basic. Legacy plans run until renewal. Legacy and new plans both work with Postman v11 and v12. — [Postman Docs: About plans](https://learning.postman.com/docs/billing/about-plans)
- The Free plan is now limited to one user and cannot create a team. Before, Free plans could invite up to three users. — [Postman Docs](https://learning.postman.com/docs/billing/about-plans); per Yaak, the change took effect "March 1, 2026" and "Legacy plans convert to the new pricing at renewal", with conversions continuing through 2027 — [Yaak blog: Postman's Free Plan Is Now One User (Sep 12, 2026)](https://yaak.app/blog/postman-free-plan-one-user)
- Postman reversed several consumption meters in March 2026. Mock server usage is now "unlimited on all plans … no longer charged by consumption", collection runs are unlimited on all plans (before, they were limited by plan, with unlimited runs sold as an add-on), and Custom Domains are unlimited on Solo, Team and Enterprise. — [Postman Docs](https://learning.postman.com/docs/billing/about-plans)
- Legacy add-ons included Postman AI, Collection Runner, Partner Editor, Advanced Security Administration, API Builder, Flows and Postbot. This shows how far the product had been split into paid add-ons before 2026. — [Postman Docs](https://learning.postman.com/docs/billing/about-plans)
- Current pricing: Free $0, Solo $9/mo billed annually, Team $19/user/mo billed annually, Enterprise contact sales. AI credits are 50 or 100/month on Free (the page contradicts itself), 400/mo on Solo with $0.05/credit overage, 400/user/mo on Team with $0.04 overage, and 800/user/mo pooled on Enterprise with $0.035 overage. Automation Flows get 25, 50, 100 and 500 credits/mo by tier. Cloud performance testing costs $0.04/VuH and is unavailable on Free. Local performance testing is unlimited. "Native Git" is on all plans. — [Postman Pricing](https://www.postman.com/pricing/)
- Postman Native Git is described as "Local by default", with "One YAML file per request, per spec, per environment, under /postman in your repo". You branch, diff and review with your own git tools, run tests in CI with the Postman CLI, and publish to "Cloud View" on merge. — [Postman Native Git](https://www.postman.com/product/native-git/). Yaak's comparison post says Postman "only added Git-native file storage in 2026". — [Yaak blog index](https://yaak.app/blog)
- Sources disagree on Enterprise price and on Team monthly billing. Apidog (a competitor) reports Enterprise at $49/user/mo and a 5-person Team at $1,140/yr. — [Apidog blog, Sep 8 2026](https://apidog.com/blog/postman-free-plan-2026-one-user-limit/). A syndicated press piece claims Team costs $23/user/mo, presumably billed monthly. It is unverified and is Apidog-sponsored content. — [Barchart/Apidog press release](https://www.barchart.com/story/news/1207778/best-postman-alternatives-2026-where-to-go-after-postman-changes-pricing)
- The pricing change set off a wave of "Postman alternatives 2026" content from competitors: Apidog, Requestly, Yaak and devtoolpicks. New entrants such as Echolon, launched Feb 2026, pitch themselves as "open source, git-native, zero login". — [Yaak](https://yaak.app/blog/postman-free-plan-one-user); [Echolon writeup](https://chatgate.ai/fr/post/echolon); [Indie Hackers "100% local, no login" VS Code client](https://www.indiehackers.com/post/i-built-a-free-postman-alternative-for-vs-code-100-local-no-login-NT2ZSWOtHp3QWU6aU9FV)

**Insomnia / Kong**
- September 2023: a Hacker News headline read "Insomnia update locks out prev user data, forces account creation w/o warning". A Kong/insomnia GitHub discussion was opened on removing the forced login. Commenters said v8.3 restored offline mode, but trust did not come back. — [V2EX thread (Oct 2023, Chinese)](https://www.v2ex.com/t/979944); [Kong/insomnia issue](https://redirect.github.com/Kong/insomnia/issues/7358) (pre-2025, historical)
- Current plans: Essentials $0 (Git Sync limited to 3 users, 1,000 mock requests/mo, unlimited local and cloud projects, "Scratch Pad" free forever and local-only), Pro $12/user/mo (10,000 mock requests), Enterprise $45/user/mo (unlimited mock requests). Enterprise admins can force projects to be stored locally, in Git, or in the cloud. — [Insomnia Pricing](https://insomnia.rest/pricing)

**Bruno / Yaak / Hoppscotch (file-first or open-source challengers)**
- Bruno: Open Source $0, Pro $6/user/mo and Ultimate $11/user/mo, both billed annually. Private git providers, unlimited workspaces (Free gets 2) and unlimited OpenAPI syncs (Free gets 5/mo) are paid. Bruno claims Ultimate "can save teams more than 70%" compared with Postman. The current page no longer lists the old one-time "Golden Edition". — [Bruno Pricing](https://www.usebruno.com/pricing). Third-party pages still quote a $19 one-time Golden Edition, which appears outdated. — [AISoTools](https://aisotools.com/pricing/bruno)
- Yaak, built by the original creator of Insomnia: the source is MIT-licensed, personal use is free, and commercial use of the signed binaries needs a license on an honor system. It costs $79/yr per individual, $399 for an individual lifetime license, or $149/user/yr for Business, billed annually only. When a subscription ends, the user keeps a fallback license for the last version released while it was active. — [Yaak Pricing](https://yaak.app/pricing); [Yaak: commercial use](https://yaak.app/blog/commercial-use). Yaak's revenue grew from $160 MRR at the license launch (Dec 2024) to roughly $5k MRR (Sep 2026). The post was titled "10x growth" at the 2025 year-end. — [Yaak blog index](https://yaak.app/blog)
- Hoppscotch: Free $0, Business $6/user/mo and Enterprise $11/user/mo, both billed annually. Business adds team workspaces, docs publishing, API mocking and MCP server support. Enterprise adds SSO, SCIM, audit logs and AI agent features. — [Hoppscotch Pricing](https://hoppscotch.com/pricing)

**Thunder Client (VS Code extension), paywall backlash**
- Thunder Client moved features behind a Pro paywall: git sync, team sharing and the CLI runner. Free collections moved into VS Code extension storage instead of the project. Sources disagree on timing (Aug 3, 2025 vs April 2023), and the accounts come only from competitors. — [EchoAPI](https://www.echoapi.com/blog/say-goodbye-to-thunder-client-fees-export-your-projects-to-echoapi-with-ease/); [Apidog](https://apidog.com/blog/thunder-client-pro-paywall-alternatives/). Earlier, it had been praised for paid tiers at $3/user/mo versus Postman's $15. — [Atomic Object (pre-2025, possibly outdated)](https://spin.atomicobject.com/thunder-client/)

**Acquisitions and consolidation**
- RapidAPI: in November 2024 Nokia bought Rapid's API hub and marketplace plus its R&D unit, to link it to Nokia's "Network as Code" 5G platform. Terms were not disclosed. The hub was repositioned toward telecom network APIs, not developer tooling. — [TYN Magazine](https://tynmagazine.com/nokia-strengthens-api-push-with-rapid-buy/); [ARY News](https://arynews.tv/nokia-buys-worlds-largest-api-hub)
- Requestly: acquired by BrowserStack on May 6, 2025 (price undisclosed). It is promised to remain open source and independent, with interception extended to Android emulators and iOS simulators. The company claims 200,000+ developers and 10,000+ companies. — [BrowserStack press release](https://www.browserstack.com/press/browserstack-acquires-requestly-a-ycombinator-startup-for-http-interception-mocking-debugging-network-requests)

**Adjacent "swiss-army" and bundle products**
- HTTP Toolkit: the Hobbyist tier is free and includes manual breakpoints. Pro gates mocking rules, "Creating rules directly from real traffic", HAR and code export, resend/edit, and docs/validation for 2,600+ APIs. Team pricing is quote-only, and no dollar amounts were rendered on the page. — [HTTP Toolkit Pricing](https://httptoolkit.com/pricing/)
- DevToys: a free, open-source, cross-platform desktop app with "30 default offline tools" (JSON↔YAML, Base64, JWT encoder/decoder, text comparer, hashes, etc.) and an SDK for extensions published on NuGet. — [DevToys](https://devtoys.app/)
- Raycast: free core (clipboard history, snippets, window management, thousands of extensions). Paid plans are mostly about AI: Pro $10/mo ($96/yr, 500 AI credits), Plus $20/mo, Max $50/mo. — [Raycast Pricing](https://www.raycast.com/pricing)
- Warp: repositioned as an AI-agent platform. Free $0, Build $20/mo ($18 annual) including $20 of agent usage, Max $200/mo, Business $50/user/mo. — [Warp Pricing](https://www.warp.dev/pricing)
- Setapp (subscription bundle): Mac $14.99/mo, Mac+iOS $18.99/mo, Power User $22.99/mo, each with AI credits, for "hundreds" of apps. — [Setapp Pricing](https://setapp.com/pricing)
- JetBrains DataGrip is "free for non-commercial use". It now connects AI agents (Junie, Claude, Codex, Copilot, Gemini CLI, any ACP agent) with schema context and read-only modes, and ships an MCP server. — [DataGrip](https://www.jetbrains.com/datagrip/)
- Keploy (Apache-2.0 core) records real traffic with eBPF, converts it into tests plus dependency mocks (DB, queues, external APIs) and replays them in CI. It claims 300M+ mocks created and shows about 18.7K next to its GitHub link. — [Keploy](https://keploy.io/)

### Inferences
- What backfired, in a pattern: (1) forced login or cloud on a previously local tool (Insomnia 2023); (2) paywalling features that used to be free, especially storage location (Thunder Client); (3) removing free team collaboration (Postman 2026); (4) consumption metering on mocks and runs, which Postman reversed in 2026, suggesting it was unpopular or uncompetitive. Every one of these produced a lasting wave of "alternatives" content.
- What worked: files on disk plus git (Bruno, Yaak, now Postman), honest licensing (Yaak's honor-system commercial license grew about 30x in MRR in ~21 months), and free personal or hobby tiers with commercial thresholds (Lens, OrbStack, DataGrip).
- Postman's Native Git weakens "files in git" as a unique selling point for /dev/all. The project-centric cross-category integration and the local-first relay/history are still open.
- The AI layer is being monetized everywhere through credits (Postman, Copilot, Raycast, Warp, Setapp). This is a new kind of subscription fatigue that a local-LLM-first design avoids.

### Gaps
- The SmartBear acquisition of Stoplight (widely reported in 2023) and the fate of RapidAPI's desktop client (formerly Paw) were not re-verified in this session.
- Apidog's official pricing page did not render, so exact Apidog prices are unverified.
- The Postman v12 release date and full v12 feature list could not be confirmed. The release-notes page rendered empty, and docs only reference "v11 or v12".
- No direct HN or Reddit thread content was retrieved because the search budget was exhausted.

---

## 2. What developers pay today: an illustrative "stack cost" table (prices checked 2026-10-21)

### Takeaway
A single commercial developer who pays separately for the categories /dev/all targets spends roughly $490-$1,000 in year one. A 5-person team spends roughly $5,700/yr (about $1,130/dev) before AI assistants. Recurring SaaS fees for tunnels, mocks, webhooks and load testing are the stickiest part. History retention (24h-30 days) and request quotas are the main levers vendors use to push upgrades.

### Cited Findings: price points

| Category | Tool / tier | Listed price (source page, 2026-10-21) | Annualized, 1 seat | /dev/all replaces? |
|---|---|---|---|---|
| API client | Postman Solo / Team | $9/mo; $19/user/mo (annual) — [Postman](https://www.postman.com/pricing/) | $108 / $228 | Yes |
| API client | Insomnia Pro / Enterprise | $12 / $45 per user/mo — [Insomnia](https://insomnia.rest/pricing) | $144 / $540 | Yes |
| API client | Bruno Pro / Ultimate | $6 / $11 per user/mo, annual — [Bruno](https://www.usebruno.com/pricing) | $72 / $132 | Yes |
| API client | Yaak Individual / Business | $79/yr; $399 lifetime; $149/user/yr — [Yaak](https://yaak.app/pricing) | $79 / $149 | Yes |
| API client | Hoppscotch Business / Ent | $6 / $11 per user/mo, annual — [Hoppscotch](https://hoppscotch.com/pricing) | $72 / $132 | Yes |
| Mock server | Beeceptor Individual / Team / Scale | $10 / $25 / $99 per mo; 20% off annual; Free = 50 req/day, endpoints deleted after 30 days — [Beeceptor](https://beeceptor.com/pricing/) | ~$96 (computed) | Yes |
| Mock server | Mockoon Cloud Solo / Team | $15/mo; $29/seat/mo — [Mockoon](https://mockoon.com/pricing) | $180 / $348 | Yes (desktop app is free) |
| Mock server | WireMock Cloud | Free 1,000 calls/mo, 3 mock APIs, 10 rps; mocks stop at quota; Enterprise by quote — [WireMock](https://www.wiremock.io/pricing) | quote | Yes |
| Tunnel | ngrok Hobbyist / Pay-as-you-go | $10/mo (annual "save $24"); $20/mo + usage. Free = 1 GB, 20k req, 24h inspection retention; paid = 72h — [ngrok](https://ngrok.com/pricing) | $96 / $240+ | Yes (needs relay) |
| Tunnel alt. | Pinggy Pro / LocalXpose Pro / Tailscale | $2.50/mo annual; $8/mo; Funnel free on Personal — [devtoolpicks, Sep 2026](https://devtoolpicks.com/blog/best-ngrok-alternatives-indie-hackers-2026) | $30 / $96 / $0 | — |
| Webhook infra | Hookdeck Developer / Team | $0 (10k events, 3-day retention, 1 user); Team from $39/mo (7-day retention); Growth from $499/mo — [Hookdeck](https://hookdeck.com/pricing) | $0 / $468 | Partly (dev loop) |
| Load testing | Grafana Cloud k6 Free / Pro | 500 VUh free; Pro $19/mo incl. 500 VUh then $0.15/VUh; Enterprise ≥ $25k/yr — [Grafana](https://grafana.com/pricing/) | $228+ | Partly (local load) |
| Load testing | Postman cloud perf | $0.04/VuH; local perf unlimited — [Postman](https://www.postman.com/pricing/) | usage | Partly |
| Traffic proxy | Proxyman Standard / Team | $89 one-time (1 yr updates, $49 renew); Team $12/seat/mo subscription (min 5) — [Proxyman](https://proxyman.com/pricing) | $89 yr1 | Partly |
| Traffic proxy | Charles | $50/license per major version (v5 released 12 Mar 2025) — [Charles](https://www.charlesproxy.com/buy/) | $50 | Partly |
| Traffic proxy | HTTP Toolkit Pro | price not rendered; 25% off yearly — [HTTP Toolkit](https://httptoolkit.com/pricing/) | n/a | Partly |
| DB GUI | TablePlus Basic / Team | $99 one-time (1 device, 1 yr updates, $59 renew); Team $79/seat (min 3) — [TablePlus](https://tableplus.com/pricing) | $99 yr1 | Lightweight only |
| DB GUI | DataGrip | free non-commercial; commercial price not captured — [DataGrip](https://www.jetbrains.com/datagrip/) | n/a | Lightweight only |
| K8s GUI | Lens Plus / Pro / Enterprise | $25/mo annual ($300/yr); $25/seat; $50/seat. Free only if org < $10M revenue/funding — [Lens](https://lenshq.io/pricing) | $300 | Viewer only |
| K8s dev | mirrord Team | $50/seat/mo — [MetalBear](https://metalbear.com/mirrord/pricing/) | $600 | No (adjacent) |
| Containers | Docker Pro / Team / Business | $9 / $15 / $24 per user/mo annual — [Docker](https://www.docker.com/pricing/) | $108-$288 | No (viewer only) |
| Containers | OrbStack Pro | $8/user/mo; free = personal non-commercial — [OrbStack](https://orbstack.dev/pricing) | $96 | No |
| Utilities | DevToys / Raycast Pro / Setapp | Free / $96/yr / $14.99/mo — [DevToys](https://devtoys.app/); [Raycast](https://www.raycast.com/pricing); [Setapp](https://setapp.com/pricing) | $0-$180 | Yes (utilities) |
| AI assistant | GitHub Copilot Pro / Pro+ / Max | $10 / $39 / $100 per mo; AI credits at $0.01 each — [GitHub](https://github.com/features/copilot/plans) | $120-$1,200 | Complement (local LLM) |
| AI terminal | Warp Build / Business | $18/mo annual; $45/user/mo annual — [Warp](https://www.warp.dev/pricing) | $216 / $540 | No |

### Illustrative stack totals (author's calculations from the prices above)
- **Lean individual, commercial use, year 1, only categories /dev/all replaces:** Postman Solo $108 + ngrok Hobbyist $96 + Beeceptor Individual ~$96 + TablePlus $99 + Proxyman $89 = **~$488**. Adding k6 Pro ($228 minimum) and Lens Plus ($300, if employer > $10M) gives **~$1,016**.
- **5-person team, per year:** Postman Team $1,140 + ngrok Pay-as-you-go $240 (base) + Beeceptor Team $300 + Hookdeck Team $468 + k6 Pro $228 + Lens Pro $1,500 + Docker Team $900 + TablePlus Team $395 + Proxyman Team $495 = **~$5,666/yr (~$1,133 per dev)**, before AI. Copilot Pro for 5 adds $600. Docker and Lens licensing are compliance-driven and only partly displaced by a viewer.
- For comparison, the file-first API clients cost $72-$149/seat/yr (Bruno, Yaak, Hoppscotch).

### Inferences
- Paid tiers in tunnel, mock and webhook tools are driven mostly by (a) persistent URLs or custom domains, (b) request quotas, and (c) history retention. Examples: ngrok 24h→72h, Hookdeck 3→7→30 days, Beeceptor endpoints deleted after 30 days, WireMock mocks "stop running" at quota. A local-first app keeps unlimited history in project files, so only the public relay costs money. That relay is the natural paid component.
- Relay pricing is under heavy pressure from free options (Cloudflare Tunnel, Tailscale Funnel, zrok) and from Pinggy at $2.50/mo. Paid relay pricing should probably sit at or below ngrok Hobbyist ($8-10/mo), or be bundled into a commercial license.
- AI credits are now a separate meter in Postman, Copilot, Raycast, Warp and Setapp. Bring-your-own or local LLM removes a meter that competitors depend on.

### Gaps
- No per-seat commercial prices were captured for DataGrip, Apidog, HTTP Toolkit Pro or WireMock Cloud paid tiers.
- Postman's Team monthly price ($23 claimed) and Enterprise ($49 claimed) are only reported by competitors.
- No survey data was found on how many of these tools an average developer actually pays for. The totals above are illustrative, not measured.

---

## 3. Developer sentiment: surveys and community themes

### Takeaway
AI use is near-universal in 2025-2026 (84-89% in the major surveys), but trust is conditional ("verify first"), and privacy and security remain a real minority blocker. Collaboration and documentation problems around APIs are the norm (93% report blockers). Community sentiment after Postman's 2026 change centers on no-login, local files, git and price. Very few hard numbers exist on "too many subscriptions" specifically.

### Cited Findings
**Stack Overflow Developer Survey 2025** (49,000+ respondents, released Jul 29, 2025)
- 84% use or plan to use AI tools (2024: 76%). 46% distrust AI output accuracy (2024: 31%). — [Stack Overflow press](https://stackoverflow.co/company/press/archive/stack-overflow-2025-developer-survey/)
- Trust in AI accuracy fell to 29% per Stack Overflow's blog, or 33% with 3% "highly trust" per Tech.co. Favorability fell from 72% to 60%. 66% cite "AI solutions that are almost right, but not quite". — [Stack Overflow blog](https://stackoverflow.blog/2025/07/29/developers-remain-willing-but-reluctant-to-use-ai-the-2025-developer-survey-results-are-here/); [Tech.co](https://tech.co/news/survey-developers-using-ai-more-trust-less) (sources disagree on the trust figure because of metric definitions)

**Stack Overflow Developer Survey 2026** (30,903 responses, 169 countries)
- 80% use AI at least one hour/day. 62% are favorable. 48% trust AI "when they can easily validate" its answers. 93% say citing sources matters for verification. — [SO 2026 overview](https://survey.stackoverflow.co/2026/); [SO 2026 AI](https://survey.stackoverflow.co/2026/ai)
- Coding assistants and agents are the top AI use (66%). 73% use them daily, and 31% of daily users spend 4+ hours/day. Claude Code (66%) and GitHub Copilot (59%) lead coding agents. — [SO 2026 AI](https://survey.stackoverflow.co/2026/ai)
- 13.0% cite privacy or security as a reason to avoid AI at work. 28.7% cite security and privacy requirements as factors in AI adoption, and 22.3% cite legal or compliance. — [SO 2026 AI](https://survey.stackoverflow.co/2026/ai); [SO 2026 overview](https://survey.stackoverflow.co/2026/)
- "27.0% say the price is acceptable for the value provided" and "18.6% are building their own tools to replace paid vendors (over half using AI to help)". The exact question context was not visible and may relate to AI and vendor purchasing. — [SO 2026 overview](https://survey.stackoverflow.co/2026/)
- Database usage: PostgreSQL 57.9%, Redis 28.3%, MongoDB 19.0%. This validates a Postgres-first, Redis, Mongo set of lightweight clients. — [SO 2026 Technology](https://survey.stackoverflow.co/2026/technology)
- Data conflict: the overview page as parsed said "87% have some level of trust … up from 31%", but the AI page shows no such figure. Treat it as unverified.

**JetBrains State of Developer Ecosystem 2025** (24,534 devs, 194 countries, Apr-Jun 2025)
- 85% regularly use AI tools and 62% rely on at least one AI coding assistant. ChatGPT is at 41% (2024: 49%). Only 44% say AI is fully or partially adopted in workflows. The share saving 8+ hours/week rose from 9% to 19%. 23% worry about low-quality code. About 15% have not adopted AI, often over data protection or quality. — [JetBrains Research blog](https://blog.jetbrains.com/research/2025/10/state-of-developer-ecosystem-2025/); [The Register](https://www.theregister.com/2025/10/21/massive_jetbrains_dev_survey) (notes sample bias toward JetBrains users)

**Postman 2025 State of the API** (5,700+ respondents, Oct 2025)
- 82% have some level of API-first adoption and 25% are fully API-first. 93% of teams face collaboration blockers. 55% struggle with inconsistent documentation, which the report attributes to documentation being scattered across too many tools. 34% can't find existing APIs. 69% spend 10+ hours/week on API-related tasks. — [Postman State of the API](https://www.postman.com/state-of-api/)
- Testing coverage: functional 67%, integration 67%, performance 57%, contract testing only 17%. — [Postman State of the API](https://www.postman.com/state-of-api/)
- 89% use gen-AI daily, but only 24% design APIs for AI agents. The top security worry is unauthorized or excessive agent API calls (51%), then sensitive-data access (49%) and credential leaks (46%). About 70% are aware of MCP, 10% use it regularly, and 24% plan to explore it. — [Postman State of the API](https://www.postman.com/state-of-api/); [BusinessWire](https://www.businesswire.com/news/home/20251008162423/en)

**Community and indie themes**
- When Yaak's creator ran an RFC on cloud sync, users rejected it in favor of local files that work with Git or Dropbox ("No one wants cloud sync"), so it was dropped. — [Yaak: commercial use](https://yaak.app/blog/commercial-use)
- "100% local, no login" and "git-native, zero login" are now headline positioning for new API clients. — [Indie Hackers](https://www.indiehackers.com/post/i-built-a-free-postman-alternative-for-vs-code-100-local-no-login-NT2ZSWOtHp3QWU6aU9FV); [Echolon](https://chatgate.ai/fr/post/echolon)
- Paywall changes are framed as "bait-and-switch" or "betrayal" by critics, though these accounts come from competitors. — [EchoAPI](https://www.echoapi.com/blog/its-time-to-ditch-the-thunder-client-vscode-extension/)

### Inferences
- The trust data (verify-first, "almost right") argues for AI features that are grounded in project artifacts: actual requests, responses, logs and schemas. It argues against free-form generation.
- The privacy and compliance blockers (13% avoid AI; 28.7% cite security and privacy requirements) are a real but minority segment. "Local LLM by default, cloud LLM optional" captures them without alienating the majority.
- The 17% contract-testing figure, against 67% functional and integration testing, points to an underserved workflow: detecting drift between spec, mock and live API.

### Gaps
- Direct HN and Reddit thread quotes ("too many subscriptions", "I just want a tool without login") could not be retrieved because the search budget was exhausted.
- No survey measures how many paid dev-tool subscriptions individual developers hold, or their willingness to pay for consolidation.
- No 2026 JetBrains or 2026 Postman State of the API report was confirmed as published.

---

## 4. Local-first / file-over-app movement and how it applies to dev tools

### Takeaway
The local-first principles (Ink & Switch, 2019) and file-over-app (Steph Ango, 2023) are now standard positioning for dev tools. The evidence: Bruno's plain-text files, Yaak dropping cloud sync, Insomnia's local/git/cloud choice, and Postman's "Local by default" Native Git. /dev/all's "everything is a file in git" model matches where the market went, so it needs differentiation beyond files.

### Cited Findings
- Ink & Switch's seven ideals (Kleppmann, Wiggins, van Hardenberg, McGranaghan, 2019): no spinners, multi-device, network optional, seamless collaboration, the Long Now, security and privacy by default, and ultimate ownership and control. Cloud apps make users "borrowers of our own data", and service shutdowns can make data inaccessible (the paper cites Parse's 2017 shutdown). (pre-2025, foundational) — [Ink & Switch: Local-first software](https://www.inkandswitch.com/essay/local-first/)
- File over app (Steph Ango, Obsidian CEO, Jul 1 2023): files outlast apps, so toolmakers should accept that software is temporary and give users ownership. In his words, "Apps are ephemeral, but your files have a chance to last." (pre-2025) — [stephango.com](https://stephango.com/file-over-app)
- Yaak stores data in local files so it integrates with Git or Dropbox, and calls itself "local-first". — [Yaak](https://yaak.app/blog/commercial-use)
- Postman Native Git lets you edit local files, run collections against a local service, and hold changes off the cloud until you push. — [Postman Native Git](https://www.postman.com/product/native-git/)
- WireMock persists recorded stubs as JSON files in a `mappings` directory, with bodies optionally extracted to files. File-based mock definitions are an established convention. — [WireMock record & playback](https://wiremock.org/docs/record-playback/)
- DevToys' default tools work offline and it markets itself as "privacy-focused". — [DevToys](https://devtoys.app/)

### Inferences
- The local-first ideals that /dev/all can still claim, and that git-native API clients do not fully cover: "network optional" for webhooks and tunnels (local capture history; only the relay is online) and "the Long Now" (open, documented file formats for every module: mocks, load-test scenarios, DB query snippets, k8s contexts).
- Compatibility beats a proprietary format. Reading and writing existing formats (OpenAPI, .http/REST Client, Bruno .bru, Postman YAML, WireMock mappings JSON, k6 JS, HAR) lowers switching costs and builds trust.
- Local-first does not mean "no collaboration". The git repo is the collaboration layer, which fits the "seamless collaboration" ideal at no server cost.

### Gaps
- lofi.so and Local-First Conf materials were not retrieved. No dev-tool-specific local-first adoption statistics were found.

---

## 5. Unmet needs: workflows that still require 3+ tools

### Takeaway
The highest-friction workflows cross the boundary between local and public (webhooks, tunnels), between captured and reproduced (traffic, mocks, replay), and between cluster and laptop (k8s port-forward to API client env). Each step is sold by a different vendor with its own login, retention window and quota. Vendor docs themselves prescribe multi-tool chains.

### Cited Findings
- **Webhook integration debugging.** Stripe's own docs say registered endpoints "must be publicly accessible HTTPS URLs". For localhost they suggest "a tunnelling tool such as ngrok" or the Stripe CLI (`stripe listen --forward-to localhost:4242/webhook`, `stripe trigger payment_intent.succeeded`). Manual resend works for 15 days in the Dashboard or 30 days via `stripe events resend`. Signature verification needs the unmodified raw body, and a framework that alters it breaks verification. Stripe does not guarantee event ordering and may deliver duplicates. — [Stripe Docs: Webhooks](https://docs.stripe.com/webhooks). In practice that chain is provider CLI or tunnel, then a local handler, then logs, then a dashboard for delivery status, then a DB check. Each provider (Stripe, GitHub, Shopify…) has its own variant.
- **Webhook inspection is retention-limited and quota-limited in SaaS.** Hookdeck's free tier gives 10k events, 3-day retention and 1 user, and the dashboard locks if you exceed it. — [Hookdeck](https://hookdeck.com/pricing). ngrok traffic inspection keeps 24h on free and 72h on paid. — [ngrok](https://ngrok.com/pricing). Beeceptor deletes unclaimed endpoints after 30 days. — [Beeceptor](https://beeceptor.com/pricing/)
- **Reproducing production bugs (capture → mock → replay).** WireMock supports proxy recording and snapshotting into stub files, including "repeatsAsScenarios" to replay sequences. — [WireMock](https://wiremock.org/docs/record-playback/). HTTP Toolkit sells "Creating rules directly from real traffic" as a Pro feature. — [HTTP Toolkit](https://httptoolkit.com/pricing/). Keploy built a company on record-traffic → tests + dependency mocks → replay in CI, claiming 300M+ mocks. — [Keploy](https://keploy.io/). These are three separate tools for one conceptual workflow.
- **Working against Kubernetes from a laptop.** mirrord charges $50/seat/mo to connect local processes and AI agents to a real cluster's APIs and databases. — [MetalBear](https://metalbear.com/mirrord/pricing/). Lens charges $25-50/seat/mo for cluster GUIs at larger orgs. — [Lens](https://lenshq.io/pricing). Teams pay for this pain.
- **Onboarding to a service.** Bruno gates OpenAPI syncs (5/mo free). — [Bruno](https://www.usebruno.com/pricing). 34% of teams can't find existing APIs and 55% cite inconsistent documentation spread across too many tools. — [Postman State of the API](https://www.postman.com/state-of-api/)
- **Performance testing is common but split from the API client.** 57% do performance testing. — [Postman State of the API](https://www.postman.com/state-of-api/). Cloud load testing is metered per virtual-user-hour: $0.15/VUh on k6 Pro, $0.04/VuH on Postman. — [Grafana](https://grafana.com/pricing/); [Postman](https://www.postman.com/pricing/)
- **AI-agent safety concerns.** 51% worry about unauthorized or excessive agent API calls and 46% about credential leaks. — [Postman State of the API](https://www.postman.com/state-of-api/). DataGrip adds read-only mode and confirmation dialogs for agents. — [DataGrip](https://www.jetbrains.com/datagrip/)

### Inferences
- Typical tool-hops today (inferred from the vendor workflows above):
  - Webhook bug: provider dashboard → ngrok/provider CLI → inspector → HTTP client to replay → app logs → DB GUI (5-6 tools).
  - Prod bug repro: proxy (Proxyman/Charles/HTTP Toolkit) → export HAR → WireMock/Mockoon to build stubs → API client to replay → load tool to stress-test (4-5 tools).
  - New-service onboarding: OpenAPI viewer → API client import → mock server → DB seed script or fake-data tool → kubectl port-forward / Lens (5 tools).
- Common failure modes the toolkit could fix: lost context between tools (copy/paste of URLs, tokens, IDs), ephemeral history (retention windows), environment drift (port-forward ports and tokens hand-copied into API client envs), and signature/raw-body bugs that no inspector checks.

### Gaps
- No published time-and-motion study quantifies context-switch cost for these specific workflows. First-person blog posts, talks and HN threads describing them could not be retrieved within the search budget.

---

## 6. High-value cross-tool synergy workflows (brainstorm with validation)

### Takeaway
The defensible value of /dev/all is the joins between modules, not the modules themselves. Each workflow below is backed either by a paying market for a single-purpose piece (validation) or by a documented multi-step vendor workflow. Workflows marked [inference] lack direct evidence.

### Cited Findings (validation evidence for each workflow)
1. **Capture → Mock → Replay → Test.** Record live or proxied traffic, auto-generate mock endpoints (file-based, WireMock-compatible), then replay as regression tests. The pieces are validated separately: WireMock snapshot/record into `mappings/*.json` with scenario sequences ([WireMock](https://wiremock.org/docs/record-playback/)), HTTP Toolkit Pro "rules from real traffic" ([HTTP Toolkit](https://httptoolkit.com/pricing/)), and Keploy record → tests + mocks → CI replay ([Keploy](https://keploy.io/)).
2. **Webhook loop: relay URL → local inspector → forward to localhost → verify signature → resend → save as test fixture.** Validated by Stripe's documented ngrok/CLI-forward/trigger/resend workflow and its raw-body signature pitfall ([Stripe](https://docs.stripe.com/webhooks)), and by paid webhook infrastructure from $39/mo ([Hookdeck](https://hookdeck.com/pricing)). Differentiator: unlimited local history compared with 24h-7d SaaS retention ([ngrok](https://ngrok.com/pricing); [Hookdeck](https://hookdeck.com/pricing)).
3. **OpenAPI → requests + mocks + fake data in one step.** Import a spec, then generate a request collection, mock endpoints with schema-valid randomized data, and contract checks. Validated by Bruno charging for unlimited OpenAPI syncs ([Bruno](https://www.usebruno.com/pricing)), by Postman storing one YAML file per spec in the repo ([Postman Native Git](https://www.postman.com/product/native-git/)), and by documentation scattered across tools as a top pain (55%) ([Postman SOTA](https://www.postman.com/state-of-api/)).
4. **Contract drift detector.** Diff live responses vs OpenAPI vs mock definitions and flag drift. Validated by contract testing at only 17% adoption against 67% functional and integration testing ([Postman SOTA](https://www.postman.com/state-of-api/)). This is an underserved gap.
5. **Load test against the mock (or a captured scenario), locally.** Validated by 57% doing performance testing, by cloud VUh metering ($0.15/VUh k6, $0.04/VuH Postman), and by Postman's unlimited local performance testing, which signals that local load is now expected ([Postman SOTA](https://www.postman.com/state-of-api/); [Grafana](https://grafana.com/pricing/); [Postman](https://www.postman.com/pricing/)).
6. **K8s/Docker context → auto-configured HTTP environment.** Start a port-forward from the k8s viewer, and the API client environment's `baseUrl` and auth automatically point at it. Validated indirectly by mirrord's $50/seat price for local-to-cluster dev ([MetalBear](https://metalbear.com/mirrord/pricing/)) and Lens's paid tiers ([Lens](https://lenshq.io/pricing)). [inference: no source describes this exact join]
7. **DB schema / query result → fake-data generator schema → seed DB and mock responses consistently.** Validated by PostgreSQL 57.9%, Redis 28.3% and MongoDB 19.0% usage, which sets client priority ([SO 2026](https://survey.stackoverflow.co/2026/technology)), and by DataGrip feeding schema context to AI agents ([DataGrip](https://www.jetbrains.com/datagrip/)). [join itself is inference]
8. **Response → auto-decode JWT/Base64/timestamps inline, with expiry and clock-skew warnings.** Validated by standalone utility demand: DevToys ships a JWT decoder among 30 offline tools ([DevToys](https://devtoys.app/)); Raycast's free tier is built on utilities and extensions ([Raycast](https://www.raycast.com/pricing)). [inline integration is inference]
9. **Local-LLM "explain this failure", grounded in project artifacts.** Takes a log line, finds the matching request/response by correlation ID or timestamp, links the relevant mock or DB row, and explains with citations to those artifacts. Validated by the verify-first trust pattern (48% trust when easily validated; 93% value cited sources) ([SO 2026 AI](https://survey.stackoverflow.co/2026/ai)), the "almost right" frustration (66%) ([SO 2025](https://stackoverflow.blog/2025/07/29/developers-remain-willing-but-reluctant-to-use-ai-the-2025-developer-survey-results-are-here/)), and privacy-driven AI avoidance (13%) ([SO 2026 AI](https://survey.stackoverflow.co/2026/ai)).
10. **Expose the project to coding agents via MCP, with guardrails.** Agents (Claude Code, Copilot) can call saved requests, read mocks and query DBs read-only, with confirmation on mutating calls. Validated by MCP awareness of about 70% with 10% regular use ([Postman SOTA](https://www.postman.com/state-of-api/)), Claude Code at 66% and Copilot at 59% of coding-agent users ([SO 2026 AI](https://survey.stackoverflow.co/2026/ai)), the 51% worry about excessive agent API calls, and DataGrip's read-only and confirmation model ([DataGrip](https://www.jetbrains.com/datagrip/)). Hoppscotch already lists MCP server support in Business ([Hoppscotch](https://hoppscotch.com/pricing)).
11. **Git-reviewable everything.** Mocks, scenarios, load profiles, DB snippets and webhook fixtures all diff in PRs. Validated by Postman's move to YAML-in-repo ([Postman Native Git](https://www.postman.com/product/native-git/)), WireMock's JSON mappings ([WireMock](https://wiremock.org/docs/record-playback/)), and users rejecting cloud sync ([Yaak](https://yaak.app/blog/commercial-use)).
12. **Secret redaction before anything is saved to git.** Scrub tokens and PII from captured traffic and webhook payloads before writing files. Validated by the 46% credential-leak concern ([Postman SOTA](https://www.postman.com/state-of-api/)). [design is inference]
13. **Project dashboard.** One view showing tunnel status, mock server status, webhook last-received, port-forwards, DB connections, recent failures and token expiries per environment. [inference: no direct source; follows from #2, #6 and #8]

### Inferences
- Priority order by evidence strength and cost displacement: #2 webhook loop (displaces ngrok, Hookdeck and Beeceptor), #1 capture→mock→replay (displaces HTTP Toolkit Pro, Proxyman, WireMock Cloud and Mockoon Cloud), #3/#4 spec→mocks + drift (underserved), #10 MCP for agents (fast-growing), then #5, #6 and #7.
- Single-purpose competitors cannot easily copy #2 or #6, because each requires owning at least two modules (relay + inspector + client, or k8s + client).

### Gaps
- No user research was found that validates willingness to pay specifically for these joins. These are hypotheses to test, for example with a waitlist survey or a landing-page A/B test.

---

## 7. Positioning and business models that worked for indie / OSS dev tools

### Takeaway
Proven models for indie dev tools in 2024-2026: (a) open source, with paid commercial-use binaries on the honor system (Yaak, about $5k MRR); (b) OSS core plus paid team or cloud features (Bruno, Hoppscotch, Mockoon); (c) perpetual license with one year of updates (TablePlus, Proxyman, Charles); (d) free for personal or small orgs, paid above a revenue threshold (Lens, OrbStack, DataGrip). For /dev/all, the one component with real server cost is the public relay (tunnel/webhook URL). That is the cleanest thing to charge for, but it faces near-free competition.

### Cited Findings
- **Yaak:** MIT source with paid commercial binaries ("If Yaak helps you earn money, Yaak earns money"). Prices are $79/yr individual, $399 lifetime, $149/user/yr business. Revenue went from $160 MRR (Dec 2024) to about $5k MRR (Sep 2026). The creator considered Fair Source but kept MIT, and dropped the planned paid cloud sync after an RFC. — [Yaak: commercial use](https://yaak.app/blog/commercial-use); [Yaak Pricing](https://yaak.app/pricing); [Yaak blog](https://yaak.app/blog)
- **Bruno:** an OSS client, with Pro $6 and Ultimate $11 per user/mo (annual) that gate private git providers, workspaces, OpenAPI syncs, SSO and SLA. — [Bruno Pricing](https://www.usebruno.com/pricing)
- **Hoppscotch:** an OSS client, with Business $6 and Enterprise $11 per user/mo (annual) for team workspaces, mocking, MCP, SSO and audit. — [Hoppscotch Pricing](https://hoppscotch.com/pricing)
- **Mockoon:** free OSS since 2017, bootstrapped without VC. Revenue comes from Mockoon Cloud (Solo $15/mo, Team $29/seat/mo), GitHub Sponsors and the founder's freelance or fractional-CTO work. About 1M downloads, about 7k GitHub stars and "tens of thousands" of users. It was a GitHub Accelerator pick. — [Mockoon Pricing](https://mockoon.com/pricing); [OSC 2025 talk](https://pretalx.com/open-source-conference-2025/talk/LELLQC/); [Mockoon blog](https://mockoon.com/blog/partner-github-maintainer-month-2026/)
- **HTTP Toolkit:** free Hobbyist, with Pro gating automation (mock rules, rules from traffic, export, resend, API validation, advanced proxy config, themes). — [HTTP Toolkit Pricing](https://httptoolkit.com/pricing/)
- **Perpetual licensing still sells:** TablePlus $99 one-time with 1 year of updates and $59 renewal ([TablePlus](https://tableplus.com/pricing)); Proxyman $89 one-time with $49 renewal, and a Team subscription at $12/seat/mo ([Proxyman](https://proxyman.com/pricing)); Charles $50 per major version, with upgrades at 40% of the original price ([Charles](https://www.charlesproxy.com/buy/)).
- **Threshold or commercial-use freemium:** Lens is free unless the org has more than $10M revenue or funding ([Lens](https://lenshq.io/pricing)). OrbStack is free for personal non-commercial use and $8/user/mo for commercial use ([OrbStack](https://orbstack.dev/pricing)). DataGrip is free for non-commercial use ([DataGrip](https://www.jetbrains.com/datagrip/)).
- **Relay pricing benchmarks:** ngrok Hobbyist is $10/mo and Pay-as-you-go $20/mo + usage ([ngrok](https://ngrok.com/pricing)). Pinggy Pro is $2.50/mo annual, LocalXpose Pro $8/mo, Cloudflare Tunnel free, and zrok has a free hosted tier and is self-hostable under Apache-2.0 ([devtoolpicks, Sep 2026](https://devtoolpicks.com/blog/best-ngrok-alternatives-indie-hackers-2026)).
- **Acquisition exit path exists:** BrowserStack acquired Requestly in 2025 and Nokia acquired the RapidAPI hub in 2024. — [BrowserStack](https://www.browserstack.com/press/browserstack-acquires-requestly-a-ycombinator-startup-for-http-interception-mocking-debugging-network-requests); [TYN](https://tynmagazine.com/nokia-strengthens-api-push-with-rapid-buy/)

### Inferences
- **Recommended hybrid for /dev/all** (synthesis, not sourced):
  1. Open-source core app, fully functional offline with no login. This is required to earn trust given the Insomnia and Thunder Client history.
  2. A commercial-use license à la Yaak or OrbStack: about $79-$129/yr per individual or perpetual with one year of updates à la TablePlus. The pitch is that one license replaces about $488-$1,016/yr of separate tools (section 2 calculation).
  3. An optional hosted relay (stable subdomain, webhook capture while the laptop is offline, team-shared inbox) priced at $5-10/mo, or included in the commercial license. It should also be self-hostable to honor local-first.
  4. Never meter AI: bring your own key or use a local LLM.
- Avoid repeating Postman's mistakes: no paywall on storage location, no forced cloud, no removal of free features after launch. A public "free forever" commitment, like Mockoon's maintainer-month and open-source pledges, signals trust.
- Price anchoring: file-first API clients set a ceiling of about $72-$149/seat/yr. A toolkit that also replaces the mock, tunnel, proxy and DB GUI can justify a higher price, but only if each module is "good enough" against category leaders.

### Gaps
- No disclosed revenue was found for Bruno, Hoppscotch, HTTP Toolkit, TablePlus, Proxyman, OrbStack or DevUtils. HTTP Toolkit's business-model blog posts were not reachable (the blog index showed only one post).
- No data was found on the conversion rate of honor-system commercial licenses beyond Yaak's aggregate MRR.
- Docker Desktop's paid-use threshold was not shown on the Docker pricing page and was not verified this session.

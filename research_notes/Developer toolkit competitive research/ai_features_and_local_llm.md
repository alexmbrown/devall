# AI Features in Developer Tools & Local-LLM Runtime Options for a Rust + Tauri Desktop App (/dev/all) — as of Oct 2026

Research date: 2026-10-21. Anything dated before 2025 is flagged "(pre-2025, possibly outdated)". Model claims are dated by release/source date. Note: the web-search budget ran out partway through. Later items were checked only by fetching primary pages directly, and some planned checks (HN/Reddit sentiment, Kreya, TablePlus, HTTP Toolkit, Phi/SmolLM/Llama 4, MLC/llamafile/LocalAI) could not be done. They are listed under Gaps.

---

## 1. What AI features do competing dev tools ship, and how well do they work?

### Takeaway
By late 2026, almost every API client ships three things: (a) an in-app assistant or agent that writes tests, scripts, request bodies and mocks; (b) an MCP server so that outside agents (Claude Code, Cursor) can drive the tool; and (c) increasingly, bring-your-own-model support, including local OpenAI-compatible endpoints or GGUF files. Database and Kubernetes tools mostly do natural language → query/command, grounded in schema or cluster context. Vendors themselves warn that results can be wrong and that small local models produce worse output. I found little hard data on user satisfaction.

### Cited Findings

**API clients**
- **Postman Agent Mode** (announced at POST/CON 2025) runs locally in the desktop app. It works across collections, tests, workspaces, environments, monitors, Spec Hub and web apps. It can create and manage requests, flows and mock servers, debug errors, and write tests. — [Postman docs: Agent Mode overview](https://learning.postman.com/docs/agent-mode/overview.md); [SD Times](https://sdtimes.com/api/postman-releases-several-new-capabilities-at-its-annual-user-conference/)
- Postman Agent Mode has slash-menu "skills" (prebuilt multi-step workflow prompts with placeholders). The chat lets users switch AI models or configure an MCP server. — [Postman docs: skills](https://learning.postman.com/docs/agent-mode/skills); [Postman docs: overview](https://learning.postman.com/latest-v-12/docs/agent-mode/overview)
- **Postman Agent Mode on Cloud (beta)** runs coding tasks in an isolated cloud sandbox with a full audit trail. Runs can be started from Slack, and users can monitor long-running background agents and step in mid-run. — [Postman docs: Agent Mode Cloud](https://learning.postman.com/docs/postman-ai/agent-mode/agent-mode-cloud/overview)
- Postman's AI overview also lists: sending requests to AI models and comparing them by response, response time and token count; comparing MCP servers by tools, resources and prompts; Postman Flows AI (low-code testing of AI models and MCP servers); and `postman init`, which installs "agent skills" for coding agents. Users can "add an AI model of your choice." — [Postman docs: About Postman AI](https://learning.postman.com/docs/postman-ai/overview)
- **Postbot** (the older Postman assistant) is free on all plans but capped at 50 activities per user per month, with an add-on beyond that. It requires the v11 client and admin enablement. — [Postman: Is Postbot usage limited](https://learning.postman.com/help/faqs/postbot/is-postbot-usage-limited.md); [Postman: I can't use Postbot](https://learning.postman.com/help/resolve-issues/postbot/i-cant-use-postbot.md)
- **Postman MCP Generator**: pick requests from public APIs on the Postman API Network, and Postman generates an MCP server with one tool per request. It supports STDIO and Streamable HTTP and is downloaded as a ZIP to run locally. — [Postman docs: Generate an MCP server](https://learning.postman.com/docs/postman-ai/mcp-servers/generate/)
- **Insomnia 12** (GA Nov 4, 2025) added AI mock-server generation "from a prompt, OpenAPI definition, or live URL response" (scaffolds routes, responses, config) for self-hosted mocks. It also added AI commit suggestions (groups staged changes and drafts messages) and an MCP client. — [Kong press release](https://konghq.com/company/press-room/press-release/kong-releases-insomnia-12-to-accelerate-api-and-mcp-development-with-ai-powered-testing-and-collaboration); [Insomnia docs: AI in Insomnia](https://developer.konghq.com/insomnia/ai-in-insomnia/)
- Insomnia providers: Claude, OpenAI, Gemini, "LLM URL" (custom/proxied/self-hosted endpoint), and "Local LLM". For Local LLM, you drop a `.gguf` file into `/Insomnia/llms/` and click Activate. The docs warn that local models "may run slower and produce less-refined responses", and that models under 10B parameters "may produce less accurate commit suggestions." AI is free "as of v12" and Enterprise admins can switch AI off instance-wide. Hosted-LLM credentials are stored locally and "never synced." — [Insomnia docs: AI in Insomnia](https://developer.konghq.com/insomnia/ai-in-insomnia/)
- **Bruno v4.0.0** (listed July 30, 2026) shipped "Bruno AI" (beta): inline autocomplete, script generation, test generation and documentation generation. It uses the user's own provider keys and routes no data through Bruno's servers. Bruno reported 4M+ developers in July 2026. Pricing is unclear: directories disagree ($6/yr vs $11/user/month). — [EveryDev: Bruno AI](https://www.everydev.ai/tools/bruno-ai); [Business Wire via Rutland Herald, Jul 16 2026](https://www.rutlandherald.com/news/business/bruno-surpasses-4-million-developers-wins-api-world-2026-award-for-best-in-api-developer/article_b2f8d4d2-c274-5cca-bd9b-924fed3947a8.html) (secondary sources; official Bruno AI docs URL returned 404)
- Bruno's plain-text, git-native collections let coding agents inspect, edit and run API workflows straight from the repo. — [EveryDev: Bruno AI](https://www.everydev.ai/tools/bruno-ai)
- **Hoppscotch** "AI Experiments" (opt-in under Settings > Experiments) can rename requests to a chosen naming style, generate request bodies from constraints, generate pre-request scripts, and generate test scripts. Each one follows a "review/validate and accept" flow. There is also an experimental **AI Assistant** chat that acts on requests, responses, environments and collections, chains actions, and has `/` skills. For the Assistant, admins connect Anthropic, OpenAI, Gemini, DeepSeek, Bedrock, Azure, or "any OpenAI-compatible endpoint, including local runtimes." The in-app editing features are not available on the Self-Hosted Edition. A **Hoppscotch MCP server** is GA for Cloud and self-hosted. — [Hoppscotch docs: AI features](https://docs.hoppscotch.io/documentation/features/ai-features)
- **Apidog MCP Server** (beta) gives AI IDEs such as Cursor the API spec as a data source. It reads and caches spec data locally so assistants can generate or modify code from the spec and search it. — [Apidog docs: MCP Server](https://docs.apidog.com/apidog-mcp-server); [Apidog docs overview](https://docs.apidog.com/overview-880559m0)
- **Yaak** (open-source, local-first, Tauri-based API client) has an MCP server plugin, "still quite beta", shipped in 2026.1.0. It lets assistants list and inspect requests, send requests and read responses, create/update/delete requests, organise folders and work across workspaces. It starts with Yaak and listens on `http://127.0.0.1:64343/mcp` (Streamable HTTP). — [Yaak docs: MCP Server](https://yaak.app/docs/getting-started/mcp-server); [Yaak feedback post](https://yaak.app/feedback/posts/mcp-server-running-in-local-in-order-to-interact-with-apis-using-llm)
- Yaak users have asked for a native MCP *client* for testing MCP servers, because "the only reliable option is really MCP Inspector". Not shipped per the post. — [Yaak feedback: native MCP client](https://yaak.app/feedback/posts/native-mcp-client-for-testing-and-debugging-mcp-servers)

**Database tools**
- **MongoDB Compass** turns natural language into filter queries and aggregation pipelines (since v1.40.x, announced Sept 2023, pre-2025). It uses Azure OpenAI, and prompts plus schema details are sent to Microsoft/OpenAI. The docs call it "experimental and may give inaccurate results" and tell users to review generated syntax before running it. Rollout is gradual. — [MongoDB docs: Query with natural language](https://www.mongodb.com/docs/compass/query-with-natural-language/); [MongoDB announcement (2023)](https://investors.mongodb.com/news-releases/news-release-details/mongodb-announces-four-new-ai-powered-capabilities-improve)
- **Redis Insight – Redis Copilot** has a general docs chatbot and a context-aware data chatbot that turns plain language into search queries. It runs on OpenAI. Index schemas and sample keys may be sent to Redis's backend and OpenAI. Sign-in and terms acceptance are required. The architecture blog is from Oct 2024 (pre-2025): RAG over Redis docs using Redis vector search. — [Redis Copilot FAQ](https://redis.io/docs/latest/develop/tools/insight/copilot-faq/); [Redis blog (2024)](https://redis.io/blog/redis-copilot-built-on-redis-for-ai/)
- **DataGrip 2026.2** supports third-party coding agents (Junie, Claude Agent, Codex, GitHub Copilot) and local models. An Aug 2026 JetBrains post says DataGrip gives agents database abilities "thanks to our built-in MCP tools and skills" (text-to-SQL, schema-aware work). — [DataGrip 2026.2 help](https://www.jetbrains.com/help/datagrip/use-ai-in-editor.html); [JetBrains blog, Aug 26 2026](https://blog.jetbrains.com/datagrip/2026/08/26/ai-agents-in-datagrip/)
- DataGrip's AI Assistant does schema-aware chat and NL query writing, primarily powered by OpenAI and Google. In the 2026.1 docs, local models were limited to completion, in-editor generation and commit messages, not chat or agents. — [DataGrip AI feature page](https://jetbrains.com/datagrip/features/ai); [DataGrip 2026.1 help](https://www.jetbrains.com/help/datagrip/2026.1/ai-assistant-in-jetbrains-ides.html)
- **JetBrains AI Assistant** supports Ollama, LM Studio, and other OpenAI-compatible endpoints (llama.cpp, LiteLLM) as providers. It warns that without a JetBrains AI subscription "some features may not work properly" with third-party models. Docs conflict on whether "offline mode" exists after IDE 2025.3.1. — [JetBrains: Use custom models](https://www.jetbrains.com/help/ai-assistant/use-custom-models.html); [JetBrains: Offline mode](https://www.jetbrains.com.cn/en-us/help/ai-assistant/switching-to-offline-mode.html)
- VS Code's BYOK mode supports chat, tools and MCP with local models such as Ollama, but not local models for inline or next-edit suggestions without extra tooling. — [InfoWorld](https://www.infoworld.com/article/4186817/using-visual-studio-codes-airgapped-ai-model-mode.html)
- **DBeaver** AI (NL→SQL and AI Chat) supports OpenAI, GitHub Copilot, Azure OpenAI, Gemini and Ollama (enter the host, "Load Models", pick one). Community edition needs an AI extension. A Jan 2026 DBeaver post says SQL-dialect differences make general-purpose AI less accurate, and DBeaver compensates by adding real database context. — [DBeaver 25.2 docs](https://dbeaver.com/docs/dbeaver/25.2/AI-Smart-Assistance/); [DBeaver 25.1 docs](https://dbeaver.com/docs/dbeaver/25.1/AI-Smart-Assistance/); [DBeaver blog, Jan 22 2026](https://dbeaver.com/2026/01/22/ai-for-sql-coding-in-dbeaver/)
- **TablePlus**: a competitor's comparison page (AI2SQL, June 2026) says TablePlus has "no AI query generation". This is low reliability because the source is a competitor. — [AI2SQL vs TablePlus](https://builder.ai2sql.io/compare/ai2sql-vs-tableplus)

**Kubernetes / infra tools**
- **Lens Prism** (Mirantis, launched July 2, 2025) is an AI copilot inside Lens that "runs actual kubectl commands behind the scenes, scoped to your active cluster". It works within RBAC and the user's kubeconfig. It connects to "any OpenAI-compatible LLM", cloud or self-hosted, so it can run air-gapped. It is included in Lens Premium (Plus/Pro/Enterprise). — [Lens blog](https://k8slens.dev/blog/introducing-lens-prism); [Lens Prism docs](https://docs.k8slens.dev/k8slens/lens-prism/); [Business Wire](https://www.businesswire.com/news/home/20250702065831/en)
- **K8sGPT** runs deterministic "analyzers" that scan the cluster read-only. An LLM backend (OpenAI, Azure, or local Ollama) is optional and only explains the findings. It also ships an MCP server (`k8sgpt serve --mcp` for stdio; `--mcp-http --mcp-port 8089` for HTTP). — [K8sGPT Ollama tutorial](https://k8sgpt.ai/docs/tutorials/ollama); [K8sGPT GitHub](https://github.com/k8sgpt-ai/k8sgpt)
- **containers/kubernetes-mcp-server** has `--read-only` (no create/update/delete) and `--disable-destructive` flags. The docs recommend a dedicated kubeconfig; a SIG example pairs it with a view-only ServiceAccount. — [Glama: kubernetes-mcp-server](https://glama.ai/mcp/servers/@containers/kubernetes-mcp-server); [K8s SIG mcp-lifecycle-operator quickstart](https://mcp-lifecycle-operator.sigs.k8s.io/guides/quickstart/)

**Terminals / launchers**
- **Warp** supports BYOK for OpenAI, Anthropic and Google. Keys are stored locally in secure storage and "never synced". BYOK does not apply to Cloud Agents. Enterprise "BYOLLM" runs via AWS Bedrock. I found no official doc on local (Ollama) models. — [Warp docs: BYOK](https://docs.warp.dev/agents/inference/bring-your-own-api-key.md); [Warp docs: BYOK billing](https://docs.warp.dev/support-and-community/plans-and-billing/bring-your-own-api-key)
- **Raycast** added local models via Ollama in v1.99 (May 21, 2025). Pro is required, on Mac and Windows. Raycast reads each model's capabilities from Ollama: vision models accept images, tool-capable models work with AI Extensions, and thinking models show reasoning. At launch it warned that "tool choice and streaming for tool calls aren't supported by Ollama just yet, so AI Extensions can be a bit unreliable". Since v2.5 (Sept 28, 2026), extensions can provide their own AI models. — [Raycast manual: Local Models](https://manual.raycast.com/ai/local-models); [Raycast changelog 1.99](https://www.raycast.com/changelog/macos/1-99-0); [Raycast changelog 2.5](https://www.raycast.com/changelog/windows/2-5)

### Inferences
- **Table stakes for an API client in late 2026**: NL→request/script/test generation; AI mock generation from a prompt, an OpenAPI spec, or a captured live response (Insomnia); an MCP server that exposes collections and send-request; and BYO provider including local. Shipping without these would look dated.
- **Postman** is going for cloud agents and fleet automation. **Bruno, Yaak, Insomnia and Hoppscotch** compete on BYO-key, local-first and "no data through our servers". /dev/all fits the second group, and its differentiator should be breadth of context (HTTP + mocks + DB + K8s + logs in one project), not agent sophistication.
- **Human-in-the-loop "propose → review → accept" is the common UX** (Hoppscotch accept flow, Compass "review before running"). Combined with vendor warnings about inaccuracy, this suggests users tolerate AI as a drafting aid, not an autonomous actor.
- **Small local models underperform on free-form generation** (Insomnia's <10B warning; Raycast's tool-calling unreliability). /dev/all should lean on structured, grammar-constrained tasks where small models do well (see §4).
- **The K8sGPT pattern (deterministic analyzers first, LLM only for explanation)** fits small local models well and should be copied for load-test analysis, log analysis and failing-request diagnosis.

### Gaps
- **User sentiment** (HN/Reddit) on Postbot, Insomnia AI, Bruno AI, Compass NL queries etc. was not found: one search returned nothing relevant and the search budget then ran out. No satisfaction or accuracy metrics were found for any tool.
- **HTTP Toolkit, Kreya, TablePlus**: no reliable first-party information on AI or MCP features.
- **Apidog** in-app AI features beyond the MCP server (e.g., AI test-case generation) are unverified.
- **Postman "AI Agent Builder"** details are not separately verified beyond the AI overview page.
- **Warp's** local-model support is unconfirmed.
- The **Bruno AI official docs page** returned 404, so Bruno details rest on secondary sources.

---

## 2. MCP in 2026: should /dev/all expose an MCP server, and what are the security considerations?

### Takeaway
MCP is the standard way dev tools integrate with agents. Postman, Hoppscotch, Apidog, Yaak, K8sGPT, kubernetes-mcp-server and DataGrip all *expose* servers; Insomnia, Postman Agent Mode, Jan and mistral.rs *consume* them. The spec revision of 2026-07-28 made the protocol stateless (breaking change). /dev/all should expose a local MCP server: prefer stdio, or Streamable HTTP on 127.0.0.1 with Origin validation plus a token. Tools should be read-only by default, with explicit user confirmation for mutating actions.

### Cited Findings
- **MCP spec 2026-07-28 (current)** changes:
  - Protocol-level sessions and `Mcp-Session-Id` are removed.
  - The `initialize` handshake is removed; every request carries protocol version and client capabilities in `_meta`.
  - New mandatory `server/discover`.
  - `subscriptions/listen` replaces the GET stream and resource subscribe.
  - Tasks move to an official extension.
  - Multi Round-Trip Requests (MRTR) replace server-initiated sampling, elicitation and roots requests.
  - Roots, Sampling and Logging are deprecated.
  - Dynamic Client Registration is deprecated in favour of Client ID Metadata Documents.
  - Tool schemas may use any JSON Schema 2020-12 keywords.
  - Servers SHOULD return tools in deterministic order (helps prompt caching).
  - List results carry `ttlMs`/`cacheScope`.
  - New required `Mcp-Method`/`Mcp-Name` headers.
  - A deprecation policy with a 12-month minimum window.
  
  — [MCP changelog 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/changelog)
- Press called it the largest revision since launch (Nov 2024) and flagged backward-incompatible changes. The RC was locked May 21, 2026. Clients can negotiate down to 2025-11-25, and the Go SDK v1.7.0 supports the new version. — [The Register](https://www.theregister.com/a/5276722); [Towards AI](https://pub.towardsai.net/model-context-protocols-2026-07-28-spec-puts-governance-before-speed-2b240295a1c5); [MCP go-sdk releases](https://github.com/modelcontextprotocol/go-sdk/releases)
- **Streamable HTTP security requirements**: servers "MUST validate the `Origin` header on all incoming connections to prevent DNS rebinding attacks" (403 if invalid). "When running locally, servers SHOULD bind only to localhost (127.0.0.1)". "Servers SHOULD implement proper authentication for all connections." — [MCP spec: Streamable HTTP](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)
- **MCP Security Best Practices**:
  - Local servers SHOULD "use the stdio transport to limit access to just the MCP client". If using HTTP, "require an authorization token" or use unix domain sockets / restricted IPC.
  - Clients offering one-click local server configuration MUST show the exact command without truncation and require explicit approval.
  - Token passthrough is forbidden.
  - Scope minimisation is recommended: a minimal initial scope with incremental elevation, and no wildcard or omnibus scopes.
  
  — [MCP Security Best Practices (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices)
- **Real CVEs from local HTTP MCP servers**: the MCP TypeScript SDK before 1.24.0 (CVE-2025-66414) and Python SDK before 1.23.0 (CVE-2025-66416) did not enable DNS-rebinding protection by default. A malicious website could invoke tools on a localhost MCP server. stdio was unaffected. The Go SDK fix is in 1.4.0. — [NVD CVE-2025-66416](https://nvd.nist.gov/vuln/detail/CVE-2025-66416); [SentinelOne CVE-2025-66414](https://www.sentinelone.com/vulnerability-database/cve-2025-66414/); [GHSA advisory](https://cvepremium.circl.lu/vuln/GHSA-w48q-cv73-mx4w)
- **Official Postman MCP server** offers Minimal (default), Code, Full ("100+ tools") and Learn tool sets. It runs remote (`mcp.postman.com`, OAuth) or local (`npx @postman/postman-mcp-server`, API key). Supported clients include Claude Code, Cursor, VS Code, Codex and Copilot CLI. The README notes that the remote server cannot reach the user's machine, so the local server is recommended for testing local APIs. — [GitHub: postmanlabs/postman-mcp-server](https://github.com/postmanlabs/postman-mcp-server)
- **Other dev-tool MCP servers**: Yaak (127.0.0.1:64343, beta), Hoppscotch (GA), Apidog (beta, spec as context), K8sGPT (stdio/HTTP), kubernetes-mcp-server (`--read-only`), DataGrip built-in MCP tools. — see §1 citations: [Yaak](https://yaak.app/docs/getting-started/mcp-server), [Hoppscotch](https://docs.hoppscotch.io/documentation/features/ai-features), [Apidog](https://docs.apidog.com/apidog-mcp-server), [K8sGPT](https://github.com/k8sgpt-ai/k8sgpt), [kubernetes-mcp-server](https://glama.ai/mcp/servers/@containers/kubernetes-mcp-server), [DataGrip blog](https://blog.jetbrains.com/datagrip/2026/08/26/ai-agents-in-datagrip/)
- **MCP consumers among dev tools**: Insomnia 12 has an MCP client; Postman Agent Mode can configure MCP servers and compare them; Jan has "MCP integration for agentic capabilities"; mistral.rs is an MCP client over process, HTTP or WebSocket. — [Insomnia docs](https://developer.konghq.com/insomnia/ai-in-insomnia/); [Postman overview](https://learning.postman.com/docs/postman-ai/overview); [Jan GitHub](https://github.com/janhq/jan); [mistral.rs GitHub](https://github.com/EricLBuehler/mistral.rs)
- **Ecosystem size** (secondary, April 2026): ~20,000 servers on Glama and ~19,000 on mcp.so. The main threats listed are tool poisoning, rug-pulls and indirect prompt injection via tool outputs. — [Truto guide](https://truto.one/blog/how-to-build-mcp-servers-for-ai-agents-2026-hands-on-architecture-guide.md)

### Inferences
- **Yes, expose an MCP server.** Competitors treat it as table stakes. For a local-first toolkit it is the main way to work with Claude Code, Cursor and Codex without building a full agent. The Yaak user request and Postman's own "use local server for local APIs" note both point to demand for *local* MCP access to a dev's environment.
- **Suggested MCP surface, in tiers:**
  1. Read-only: list collections/requests/environments (secret values masked), read history and responses, query mock definitions, read logs, run regex/jq/JWT-decode/diff utilities. These are pure functions and safe.
  2. Side-effecting but local: send a request in the project, start or stop a mock, create a webhook endpoint, run a small load test with capped RPS and duration.
  3. Dangerous: DB writes, kubectl apply/delete, opening a public tunnel, high-RPS load tests. Gate these behind in-app confirmation (an MRTR elicitation is now the spec-native way) or keep them off by default, per connection.
- **Transport**: offer stdio via a small CLI shim (`devall mcp`) that talks to the running app over authenticated IPC. Also offer optional Streamable HTTP bound to 127.0.0.1 with strict Origin checks and a per-install bearer token. This avoids the DNS-rebinding CVE class.
- **Implementation**: target spec 2026-07-28 but negotiate down to 2025-11-25, because many clients will lag. Use deterministic tool ordering and per-tool annotations (readOnly/destructive) to drive UI confirmations.
- Building an in-app **MCP client and inspector** (like Postman's MCP comparison and Insomnia's MCP client) is also a strong fit for an HTTP-client product. Yaak users explicitly asked for it.

### Gaps
- I did not verify which official Rust MCP SDK (`rmcp`) version supports 2026-07-28.
- No data on how many real users connect agents to API clients via MCP, or on incidents involving dev-tool MCP servers beyond the SDK CVEs.

---

## 3. Local inference runtimes for a Tauri app: embed, sidecar, or connect to an existing endpoint?

### Takeaway
The lowest-risk architecture is layered:
1. Always support "connect to any OpenAI-compatible endpoint" (Ollama, LM Studio, llama-server, Jan, cloud).
2. For a batteries-included option, ship or download **llama.cpp's `llama-server` as a Tauri sidecar**. It is MIT-licensed, has the widest GPU backend coverage (Metal/CUDA/Vulkan/HIP/SYCL…), supports JSON-schema/GBNF constrained output and tool calling, and needs no native build work.
3. Optionally embed small, fixed-purpose models in-process: embeddings via `ort` or candle, or a pure-Rust LLM via mistral.rs.

Don't build on the candle-based `tauri-plugin-llm`: it is under a non-commercial licence and early-stage.

### Cited Findings

**llama.cpp / llama-server**
- MIT-licensed. Backends include CUDA, HIP (AMD), Metal, Vulkan, SYCL (Intel), MUSA, CANN, OpenCL (Adreno), OpenVINO (in progress), WebGPU, RPC, and an optimised CPU path (AVX/AVX2/AVX512/AMX, NEON, RVV). The repo has ~131.9k stars. — [GitHub: ggml-org/llama.cpp](https://github.com/ggml-org/llama.cpp)
- Liquid AI's deployment guide recommends the **sidecar** pattern (launch `llama-server`, talk HTTP on localhost) for "Electron, Tauri, .NET, Java, or Python" apps that want "the full feature set (chat templates, tool calling, JSON schema, multimodal, prompt cache) with zero native build work". It recommends in-process bindings only when you "cannot spawn a helper process, need the tightest latency, or want a single binary". Both use the same GGUF files. — [Liquid AI docs: llama.cpp desktop](https://docs.liquid.ai/deployment/on-device/llama-cpp/desktop)
- Sidecar operational guidance from the same guide:
  - Bundle per-platform archives (`macos-arm64`, `win-cpu-x64`/`win-cuda-*`, `ubuntu-x64`, `ubuntu-vulkan-x64`). "Metal is built into the macOS binary."
  - Poll `GET /health` until `{"status":"ok"}`, and kill the child process on app exit.
  - Download the model on first launch, not at install time.
  - In production pin the exact file (`-m`), not `-hf`.
  - Benchmark with `llama-bench`.
  
  — [Liquid AI docs: llama.cpp desktop](https://docs.liquid.ai/deployment/on-device/llama-cpp/desktop)
- Upstream warns that prebuilt binaries "may not include all GPU backends", so full GPU support may require building from source. Official Docker images include a Vulkan variant. — [llama.cpp installation guide (mirror)](https://mintlify.com/ggml-org/llama.cpp/installation)
- **Structured output**: llama-server accepts an OpenAI-style `response_format` with a JSON schema and converts it to a grammar. `/completion` takes `json_schema` directly, and hand-written GBNF goes via `grammar` or `--grammar-file`. Only a subset of JSON Schema is supported, and unsupported keywords are *ignored*, so validate output in code. The model doesn't see the schema, so describe it in the prompt too. — [Liquid AI: structured output](https://docs.liquid.ai/deployment/on-device/llama-cpp/structured-output); [llama.cpp GBNF README](https://raw.github.com/ggerganov/llama.cpp/master/./grammars/README.md)
- A version-dependent bug was reported: "Either "json_schema" or "grammar" can be specified, but not both". — [llama.cpp issue #11847](https://github.com/ggml-org/llama.cpp/issues/11847)
- An unverified third-party claim says grammar constraints slow generation by 30–80% depending on complexity. — [InsiderLLM: structured output](https://insiderllm.com/guides/structured-output-local-llms/)

**Rust in-process options**
- **llama-cpp-2** (utilityai/llama-cpp-rs) is a thin wrapper that mimics the llama.cpp API "as closely as possible". Its README says "This crate is NOT safe". It needs clang (bindgen). It has feature flags for CUDA, Metal, Vulkan, OpenCL and OpenMP. It does not follow semver meaningfully, so pin exact versions. Recent versions are ~0.1.13x–0.1.154 (registries differ). — [rust-digger: llama-cpp-2](https://rust-digger.code-maven.com/crates/llama-cpp-2); [SourcePulse: llama-cpp-rs](https://www.sourcepulse.org/projects/1842222)
- **mistral.rs**:
  - Pure Rust, MIT, latest v0.9.5 (date not shown).
  - Backends: CUDA (FlashAttention V2/V3), Metal, multi-GPU. Vulkan is *not* mentioned.
  - Quantization: GGUF 2–8-bit, GPTQ, AWQ, HQQ, FP8, BNB, and in-situ quantization (ISQ) of HF models; "45+ architectures".
  - `mistralrs serve` exposes OpenAI-compatible `/v1` and Anthropic-compatible Messages endpoints.
  - "Integrated tool calling with grammar enforcement and strict schema mode"; MCP client.
  - Embeddable via `cargo add mistralrs`.
  
  — [GitHub: EricLBuehler/mistral.rs](https://github.com/EricLBuehler/mistral.rs) (the page also names models such as "Gemma 4" and "Qwen3.8-Flash-Next"; unverified)
- **candle** (Hugging Face): Apache-2.0/MIT. Backends: CPU (MKL/Accelerate), CUDA (NCCL multi-GPU), WASM, plus Metal kernels in the repo. It uses llama.cpp quantized types and GGUF for some models. Its goal is to "make serverless inference possible" with "lightweight binaries". The model list on the README lags (e.g., Gemma v1/v2, Qwen3 MoE; no Gemma 3 listed), and the README has many build/link troubleshooting notes. — [GitHub: huggingface/candle](https://github.com/huggingface/candle)
- **tauri-plugin-llm** (CrabNebula) runs on candle with Safetensors only. Llama 3.x and Qwen3 are "Stable"; Gemma 3 is "In progress". Metal is on by default; CUDA is "not extensively tested". It handles one model at a time and is "early-stage". **License: PolyForm Noncommercial 1.0.0.** — [CrabNebula docs: tauri-plugin-llm](https://docs.crabnebula.dev/plugins/llm/)
- **ort** (ONNX Runtime bindings): Apache-2.0/MIT, docs at 2.0.0-rc.15 with ONNX Runtime 1.31.0, supports "almost any hardware accelerator". It is used by Hugging Face Text Embeddings Inference, FastEmbed-rs and Google Magika, which makes it a strong choice for embeddings and classifiers. — [GitHub: pykeio/ort](https://github.com/pykeio/ort)

**External runtimes (connect, don't bundle)**
- **Ollama**: MIT. Native REST API on `:11434`. OpenAI-compatible `/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, `/v1/responses` (non-stateful, since v0.13.3) and `/v1/models`. Supports streaming, JSON mode, tools, vision (base64 only), and reasoning control. **Unsupported**: `tool_choice`, logprobs, `n`, image URLs. Community Rust SDK: `ollama-rs`. — [GitHub: ollama/ollama](https://github.com/ollama/ollama); [Ollama docs: OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility)
- Ollama added Vulkan in 0.12.11 behind `OLLAMA_VULKAN=1` (late 2025). A 2026 guide still recommends ROCm first on AMD and says backend choice is "less explicit than in bare llama.cpp". — [Phoronix](https://www.phoronix.com/news/ollama-0.12.11-Vulkan); [Glukhov: ROCm vs Vulkan 2026](https://glukhov.org/llm-hosting/comparisons/amd-rocm-vs-vulkan-llm-hosting/)
- Ollama v0.40.0 (Sept 25, 2026): on Apple Silicon, supported architectures "will automatically run on MLX", including gemma4, qwen3.6 and qwen3.5, plus an "embeddinggemma-2" embedding model. Releases come every few days (v0.40.7 on Oct 18). The release notes also mention new "decision models" and cloud models. I could not independently verify these, and some names in the fetched summary looked odd. — [Ollama releases](https://github.com/ollama/ollama/releases)
- **LM Studio**: the `llmster` headless daemon (`lms daemon up`) is "the recommended way" to run it as a background service. A secondary source says it arrived in 0.4.0 (Jan 2026) and that the Python/TS SDKs reached 1.0.0. — [LM Studio docs: headless](https://lmstudio.ai/docs/developer/core/headless); [Kunal Ganglani blog](https://www.kunalganglani.com/blog/lm-studio-vs-ollama)
- **Jan** is a useful reference architecture: a **Tauri** desktop app with a bundled llama.cpp engine, Apache-2.0. It exposes an OpenAI-compatible local server on `localhost:1337`, has MCP integration, and downloads from Hugging Face. Engine variants: cpu, vulkan, metal, cuda12, cuda13, hip/rocm. — [GitHub: janhq/jan](https://github.com/janhq/jan)
- **Windows ML** went GA in Sept 2025 as the built-in on-device runtime across CPU/GPU/NPU. Foundry Local covers ready-made open models; ONNX Runtime GenAI is the lower-level path for custom ONNX models on NPUs (AMD guide, 2026). — [Windows blog](https://blogs.windows.com/windowsdeveloper/2025/09/23/windows-ml-is-generally-available-empowering-developers-to-scale-local-ai-across-windows-devices); [Microsoft: local AI on Windows](https://developer.microsoft.com/en-us/windows/local-ai); [AMD 2026 article](https://www.amd.com/en/developer/resources/technical-articles/2026/ai-model-deployment-using-windows-ml-on-amd-npu.html)

**Tauri sidecar mechanics**
- Add binaries under `bundle.externalBin` in `tauri.conf.json`. Each binary needs a `-$TARGET_TRIPLE` suffix (`rustc --print host-tuple`). The shell plugin is required. Grant `shell:allow-spawn`/`shell:allow-execute` with `"sidecar": true`. In Rust, use `app.shell().sidecar("name")` (filename only). Arguments can be allowlisted per capability with static strings or regex validators, and must match the declared order. — [Tauri v2 docs: sidecar](https://v2.tauri.app/develop/sidecar/)
- One Tauri 2 local-LLM app resolves the llama.cpp binary in order: env-var override (`LLAMA_CPP_BIN`), then bundled per-platform binaries. It spawns from Rust commands and streams progress events to the frontend. — [codenote.net](https://codenote.net/en/posts/tauri-cross-platform-app-with-local-llm/)

### Inferences
- **Recommended architecture for /dev/all:**
  - **Tier A (always on):** an "OpenAI-compatible endpoint" provider with auto-discovery of Ollama (:11434), LM Studio (:1234 default; not verified here), Jan (:1337) and llama-server.
  - **Tier B (one-click local):** a managed `llama-server` sidecar. Ship the CPU and Metal builds in the installer, and download the CUDA, Vulkan or HIP variant on demand after GPU detection. This keeps the base installer small; CUDA builds are large, but I have no measured size.
  - **Tier C (in-process, small fixed models):** an embedding model via `ort` (FastEmbed-rs) or candle for semantic search and PII/NER classifiers. These need no GPU and avoid process management.
- **Why sidecar over llama-cpp-2 for the main LLM:**
  - It avoids a C++ toolchain, bindgen/clang and per-GPU-backend compile matrices in CI.
  - It isolates crashes: a GPU driver fault kills the sidecar, not the app.
  - llama.cpp upgrades become binary swaps.
  - The cost is managing ports and health checks, a bigger attack surface (bind 127.0.0.1, random port, API key flag), and signing extra binaries.
- **mistral.rs** is the best pure-Rust in-process candidate (MIT, grammar-enforced tool calling, OpenAI/Anthropic-compatible server). It lacks Vulkan, though, so AMD/Intel GPU users on Windows/Linux would fall back to CPU. Treat it as an optional alternative engine, not the default.
- Avoid **tauri-plugin-llm** for a commercial or OSS-with-commercial-use product because of its PolyForm Noncommercial licence.

### Gaps
- No measured binary sizes for llama-server CPU/CUDA/Vulkan builds or for mistral.rs builds.
- MLC LLM, llamafile, LocalAI, Burn and WebGPU-in-webview options were not researched (search budget exhausted). llama.cpp lists a WebGPU backend, but its maturity was not verified.
- Licence terms for the LM Studio app (and whether `llmster` may be bundled) were not verified.
- The exact current llama.cpp official release asset list (which GPU variants ship prebuilt) was not verified.
- Ollama's "decision models" (`/v1/systemone`) feature is unverified.

---

## 4. Model choice in 2026 (≈1B–14B, plus useful larger MoE): quality, size, licences, tool calling, embeddings

### Takeaway
As of Oct 2026, the default small-model choices are:
- **Qwen3.5 small** (0.8B/2B/4B/9B; Mar 2026; Apache-2.0; 262K context; vision; strong tool-calling scores).
- **Gemma 4** (E2B/E4B, a later 12B, 26B-A4B MoE, 31B; Apr 2026; Apache-2.0 per launch coverage; built-in function calling).

For users with 16–24 GB VRAM, **Qwen3.6 35B-A3B** (MoE, 3B active) and **Qwen3.6 27B** are the top local coders. **Qwen2.5-Coder** (2024) is still recommended for fill-in-the-middle (FIM) autocomplete. For embeddings, **Qwen3-Embedding-0.6B**, **EmbeddingGemma-300m**, **nomic-embed-text** and **BGE-M3** are the practical picks. Benchmarks are mostly vendor-reported and use inconsistent harnesses.

### Cited Findings

**General-purpose small models**
- **Qwen3.5 small series** (released Mar 2, 2026): 0.8B, 2B, 4B and 9B dense models under Apache-2.0, with 262K context, native vision and hybrid thinking/non-thinking. Artificial Analysis Intelligence Index: 9B = 32 ("most intelligent model under 10B"), 4B = 27. The 4-bit 9B needs ≈6 GB and the 4B ≈3 GB. They are very verbose: 230M+ output tokens to run the index. — [Artificial Analysis, Mar 5 2026](https://artificialanalysis.ai/articles/qwen3-5-small-models); [KDnuggets](https://kdnuggets.com/best-small-language-models-on-hugging-face-right-now)
- **Qwen3.5-9B model card** (Feb/Mar 2026): hybrid Gated DeltaNet + Gated Attention architecture, Apache-2.0. Thinking mode is **on by default**; set `enable_thinking: False` to disable (no `/think` switches). Vendor-reported scores: BFCL-V4 66.1 (4B: 50.3), TAU2-Bench 79.1 (4B: 79.9), LiveCodeBench v6 65.6. Recommended frameworks are Qwen-Agent, and vLLM/SGLang with the `qwen3_coder` tool parser. — [HF: Qwen/Qwen3.5-9B](https://huggingface.co/Qwen/Qwen3.5-9B)
- Ollama sizes: Qwen3.5 4B ≈3.4 GB (Q4), 9B ≈6.6 GB (Q4). — [InsiderLLM: best local coding models (updated Aug 19 2026)](https://insiderllm.com/guides/best-local-coding-models-2026/); [InsiderLLM: Qwen 3.5 small](https://insiderllm.com/guides/qwen-3-5-small-models-9b-beats-30b/)
- **Gemma 4** (released Apr 2, 2026). Launch coverage says it moved from the custom "Gemma Terms" to **Apache-2.0**; Google's doc page links a licence page but doesn't state terms inline.
  - Sizes: E2B and E4B ("2B and 4B effective"), 12B (added later; sources conflict on timing), 26B A4B MoE, 31B dense.
  - Context: 128K (small) and 256K (medium). Text and image input for all models; audio for E2B/E4B/12B.
  - "Built-in function-calling support", configurable thinking, native system prompts.
  - Official QAT GGUF checkpoints. Q4_0 weight memory: E2B 2.9 GB, E4B 4.5 GB, 12B 6.7 GB, 26B-A4B 14.4 GB, 31B 17.5 GB (weights only, excluding KV cache).
  
  — [Google AI: Gemma core docs](https://ai.google.dev/gemma/docs/core); [DataNorth](https://datanorth.ai/news/google-releases-gemma-4-open-models); [The Decoder](https://the-decoder.com/?p=33933); [frankx.ai (12B claim)](https://frankx.ai/blog/gemma-3-analysis-2026)
- **IBM Granite 4.0**: native JSON output, tool calling (OpenAI function schema), RAG and FIM. Nano sizes are 1B and 350M. Granite 4.1 8B is listed by a third-party aggregator as released 2026-04-30 with JSON-schema-constrained decoding (unverified against IBM). — [IBM Granite docs](https://www.ibm.com/granite/docs/use-cases/prompt-engineering); [Replicate: granite-4.0-h-small](https://replicate.com/ibm-granite/granite-4.0-h-small); [modelcompare.dev](https://modelcompare.dev/models/openrouter/granite-4-1-8b)

**Coding models by VRAM tier** (InsiderLLM, updated Aug 19 2026; vendor-reported benchmarks)
- **8 GB**:
  - Qwen2.5-Coder 7B (Q4 ≈5 GB, HumanEval 88.4%) is still the best for FIM/autocomplete (2024 model, pre-2025).
  - Qwen3.5 9B (6.6 GB) is better for chat-style coding.
  - Qwen3.5 and 3.6 "are not FIM families".
- **16 GB**:
  - Qwen3.6 35B-A3B (UD-Q3_K_M 16.6 GB; UD-Q4_K_M 22.1 GB): SWE-bench Verified 73.4, LiveCodeBench v6 80.4, Apache-2.0, 262K context.
  - Qwen2.5-Coder 14B (Q4 ≈9 GB) for FIM.
- **24 GB**:
  - Qwen3.6 27B dense (Q4_K_M 16.8 GB): SWE-bench Verified 77.2, LiveCodeBench v6 83.9, no FIM.
  - Qwen3.6 35B-A3B reaches ~101.7 tok/s on an RTX 3090.
  - Devstral Small 2 (24B dense, Q4 ≈14 GB, 256K context, "agentic-tuned").
- **48 GB+**: Qwen3-Coder-Next (80B MoE, 3B active, ≈38 GB Q4, SWE-bench Verified 70.6, Apache-2.0).

— [InsiderLLM: best local coding models 2026](https://insiderllm.com/guides/best-local-coding-models-2026/)
- Other 2026 guides agree that Qwen3.6 27B is the top pick for 24 GB and that Gemma 4 E4B runs in about 3 GB VRAM. They caution that cross-vendor SWE-bench numbers aren't comparable (e.g., Gemma 4 31B listed as 52.0 on one card and 66.6 on another). — [KDnuggets: top 7 local coding models 2026](https://www.kdnuggets.com/top-7-coding-models-you-can-run-locally-in-2026)

**Tool calling benchmarks**
- **BFCL v4** (April 2026) weights agentic 40%, multi-turn 30%, live 10%, non-live 10%, hallucination 10%. — [Spheron blog](https://www.spheron.network/blog/tool-calling-benchmarks-bfcl-tau-bench-latency-optimization/)
- BenchLM's July 29, 2026 snapshot (aggregator): Qwen3.7 Max 75.0%, Qwen3.7 Plus 72.9%. Small models: LFM2.5-8B-A1B 49.7%, Mellum2-12B-A2.5B-Thinking ~45.6%, MiniCPM5-1B ~25.1%. Snapshots and aggregators conflict (another lists Llama 3.1 405B first at 88.5%). — [BenchLM: BFCL v4](https://benchlm.ai/benchmarks/bfcl-v4); [LLM Reference](https://www.llmreference.com/best/function-calling)
- Note the conflict: Qwen's self-reported BFCL-V4 66.1 for Qwen3.5-9B does not line up with BenchLM's small-model numbers (≤49.7). This is likely a harness or scoring-view difference. — [HF: Qwen3.5-9B](https://huggingface.co/Qwen/Qwen3.5-9B) vs [BenchLM](https://benchlm.ai/benchmarks/bfcl-v4)
- xLAM (1B–70B) is cited as a leading small open tool-use family, but the source's table is labelled "v3". Low confidence. — [Presenc AI](https://presenc.ai/research/open-weight-function-calling-tool-use-2026)

**Embedding models**
- **Qwen3-Embedding-0.6B**: MTEB-eng-v2 70.7, Apache-2.0, ≈1.5 GB.
- **nomic-embed-text**: ≈0.3 GB, simplest via Ollama.
- **BGE-M3**: MIT, 8K context, 100+ languages.
- **all-MiniLM-L6-v2**: ≈0.1 GB for CPU-only use.
- **NV-Embed-v2 and jina-embeddings-v3** have non-commercial licences, so avoid them.
- MTEB variants are not directly comparable.

— [D-Central: local embedding models 2026](https://d-central.tech/local-embedding-models/); [bestllmfor.com](https://bestllmfor.com/best/embeddings-self-hosted/)
- **EmbeddingGemma-300m**: 0.3B params, 768-dim output with Matryoshka truncation to 512/256/128, 2048-token context. MTEB English v2 69.67, **MTEB Code v1 68.76**. Q4_0/Q8_0 QAT variants lose <0.5 points. It has an explicit code-retrieval prompt (`task: code retrieval | query: …`) and does not support float16. Licence is the **Gemma terms (gated)**, not Apache. Paper Sept 2025. — [HF: google/embeddinggemma-300m](https://huggingface.co/google/embeddinggemma-300m)

### Inferences
- **Suggested curated "download" list for /dev/all** (as of Oct 2026, re-validate every quarter):

  | Hardware | Main model | Approx. size | Embeddings |
  |---|---|---|---|
  | ≤8 GB RAM / CPU-only | Qwen3.5-4B or Gemma 4 E4B (Q4) | ≈3–4.5 GB | EmbeddingGemma-300m or nomic-embed |
  | 16 GB unified / 8 GB VRAM | Qwen3.5-9B (Q4) | ≈6–6.6 GB | as above |
  | 16–24 GB VRAM or 32 GB+ Apple Silicon | Qwen3.6 35B-A3B (Q3/Q4) | ≈16–22 GB | as above |
  | Optional FIM autocomplete in script editors | Qwen2.5-Coder 1.5B/7B | small | — |

  - The top two tiers are Apache-2.0 per sources. Qwen3.6 35B-A3B is an MoE, so it is fast for its quality.
  - Qwen2.5-Coder is older but still the FIM recommendation.
- **Disable thinking by default** for interactive features on Qwen3.5 (thinking is on by default and very token-hungry per Artificial Analysis). Enable it only for "explain this failure" style deep analysis.
- **Always pair small models with grammar or JSON-schema constrained decoding** for structured tasks: mock data, OpenAPI fragments, regex/SQL/jq outputs, assertion lists. Validate the output in Rust (unsupported schema keywords are silently ignored by llama-server).
- **Licence hygiene**: prefer Apache-2.0/MIT models in the curated list. EmbeddingGemma's Gemma terms are gated, which matters for auto-download UX (an HF login may be required), so Qwen3-Embedding-0.6B or nomic-embed may be the friction-free default.

### Gaps
- Phi-4/Phi-4-mini, SmolLM3, Llama 4, DeepSeek distills, LFM2.5 and Mistral small/Devstral details beyond the Devstral Small 2 line were not verified (search budget exhausted). Their licences and current versions are unknown here.
- I found no independent (non-vendor) structured-output or JSON-validity benchmark for small models.
- I couldn't confirm Gemma 4's exact licence text or the 12B release date from a primary source.
- The "Qwen3.7" and "Qwen3.8" references in aggregator and guide pages are unverified and may be API-only or mis-reported.

---

## 5. Provider abstraction and model download/management UX

### Takeaway
Use an OpenAI-compatible chat interface as the internal lowest common denominator, but call Anthropic and Gemini through their native APIs. In Rust, **`genai`** (v0.7.0, Oct 19 2026; 27+ providers; MIT/Apache) is the best thin multi-provider layer. **`rig`** (v0.44, pre-1.0, MIT) suits you if you want agents, RAG and vector stores built in. **`async-openai`** gives the full OpenAI surface. For downloads, **`hf-hub` 1.0** (async, content-addressed cache, file locking, progress) plus **`llmfit`**-style hardware profiling covers "recommend a model and fetch it".

### Cited Findings
- **genai v0.7.0** (released 2026-10-19):
  - Providers include openai, openai_resp (Responses API), anthropic, gemini, vertex, bedrock, ollama, ollama_cloud, open_router, groq, deepseek, mistral, xai, together, fireworks, github_copilot and more ("27+ providers").
  - Uses "native protocols when available, OpenAI-compatible APIs when appropriate".
  - Features: streaming, tool calls with tool-choice hints, structured output with per-provider JSON-schema sanitising, images, reasoning effort, and thinking blocks preserved across tool turns.
  - Custom endpoint and auth via `ServiceTargetResolver`/`AuthResolver`, `GENAI_{n}_ENDPOINT`/`_API_KEY`, and an injectable `reqwest::Client`.
  - MIT/Apache-2.0.
  - No general embeddings API is documented.
  
  — [GitHub: jeremychone/rust-genai](https://github.com/jeremychone/rust-genai)
- Older genai docs warned that minor versions could break APIs and positioned genai as a chat-normalisation layer, deferring to `async-openai` (OpenAI) and `ollama-rs` (Ollama) for full APIs. — [docs.rs genai 0.1.7](https://docs.rs/crate/genai/0.1.7)
- **rig 0.44.0**: MIT, "pre-1.0. Releases contain breaking changes". 20+ providers and 10+ vector stores (LanceDB, Qdrant, SQLite, SurrealDB, Postgres…), FastEmbed embeddings, and a candle provider for local models. Agents with tools, OpenTelemetry GenAI conventions, `rig-cassette` record/replay, WASM. — [GitHub: 0xPlaygrounds/rig](https://github.com/0xPlaygrounds/rig)
- JetBrains (Sept 2026) describes Rig's core abstractions (provider client, completion model, agent, tools) and notes you can switch providers without rewriting LLM-dependent code. — [JetBrains Rust blog, Sept 9 2026](https://blog.jetbrains.com/rust/2026/09/09/rust-ai-in-practice/)
- **llmrust** (young crate) claims a lean core with native Anthropic/Gemini and a built-in OpenAI/Anthropic proxy. Its own README recommends genai or rig for wider coverage. — [docs.rs: llmrust](https://docs.rs/crate/llmrust/0.1.2)
- **Gaps in Ollama's OpenAI compatibility** that affect an abstraction: no `tool_choice`, no image URLs (base64 only), Responses API stateless only. — [Ollama docs: OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility)
- **hf-hub 1.0.0**: async-first (reqwest with retries), optional `blocking` feature, content-addressed cache under `HF_HUB_CACHE` with on-disk locking to deduplicate concurrent fetches, `scan_cache`, a `progress` module, Xet chunk-deduplicated transfers. The docs don't mention resumable downloads. — [docs.rs: hf_hub](https://docs.rs/hf-hub/latest/hf_hub/)
- **llmfit** (Rust): profiles CPU, RAM, GPU and VRAM, then ranks models and quantisations that fit, projecting memory use and tok/s. It reads GGUF metadata. `download` fetches GGUF from HF and "automatically selects the best quantization that fits your available hardware". It also offers a JSON API (`/api/v1/system`, `/api/v1/models`). — [docs.rs: llmfit](https://docs.rs/crate/llmfit); [llmfit docs: download](https://www.mintlify.com/AlexsJones/llmfit/api/commands/download); [Better Stack guide](https://betterstack.com/community/guides/ai/llmfit-llm/)
- **UX patterns in competitors:**
  - Insomnia: drop a `.gguf` into a folder, then Activate.
  - Raycast: Sync Models button and capability detection (vision/tools/thinking) read from Ollama.
  - DBeaver: "Load Models" from the Ollama host.
  - Jan: built-in HF model hub.
  - Warp and Insomnia: keys stored locally in OS secure storage and never synced.
  
  — [Insomnia docs](https://developer.konghq.com/insomnia/ai-in-insomnia/); [Raycast manual](https://manual.raycast.com/ai/local-models); [DBeaver docs](https://dbeaver.com/docs/dbeaver/25.2/AI-Smart-Assistance/); [Jan GitHub](https://github.com/janhq/jan); [Warp BYOK](https://docs.warp.dev/agents/inference/bring-your-own-api-key.md)
- Download the model on first launch to the user data dir and pin exact files rather than letting model-card updates change behaviour. — [Liquid AI docs](https://docs.liquid.ai/deployment/on-device/llama-cpp/desktop)

### Inferences
- **Suggested design:** a `Provider` trait with `chat`, `chat_stream`, `structured(json_schema)`, `embed` and `capabilities()`, where capabilities include tools, vision, JSON-schema, grammar, tool_choice, context length and thinking.
  - Implement it with genai for remote providers, plus a direct llama-server client for the managed sidecar, so you can use `grammar`/`json_schema` fields that the generic layer may not expose.
  - Embeddings go through a separate trait (genai lacks one): use ort/FastEmbed locally or the provider's `/v1/embeddings`.
- **Per-feature model routing:** let users assign a "fast/small" model (autocomplete, naming, classification) and a "smart" model (explanations, spec inference) separately. JetBrains and Insomnia let users assign models per feature.
- **Capability gating:** grey out features a configured model can't support (e.g., schema-constrained mock generation when the endpoint lacks JSON-schema support, or tool-calling agents on Ollama without `tool_choice`), as Raycast does.
- **Download UX:**
  - Hardware probe (llmfit-style, or embed llmfit as a library) → recommend one of three curated models → download via hf-hub with progress → verify SHA256 → register.
  - Implement resume yourself with HTTP Range if hf-hub doesn't provide it.
  - Store models in a shared cache so Ollama, LM Studio or HF-cache users aren't forced to re-download; detecting existing GGUFs in `HF_HUB_CACHE` is cheap.
- Keep API keys in the OS keychain (Tauri stronghold/keyring) and never in project files, which may be git-synced.

### Gaps
- Whether `hf-hub` 1.0 supports resumable downloads is unconfirmed.
- I did not verify `async-openai` current version or features directly.
- No first-party data on download-UX completion rates or user preferences.

---

## 6. Privacy and security: redaction, prompt injection, agentic confirmation

### Takeaway
Cloud-backed AI in dev tools routinely sends schemas and sample data to third parties: Compass sends prompts and schema to Azure OpenAI, and Redis Copilot sends index schemas and sample keys to OpenAI. A local-first tool can turn this into a selling point, but it must still:
- redact secrets before any remote call;
- treat every HTTP response, log, webhook payload and DB row as untrusted input that may carry prompt injection;
- keep AI actions to read-only or confirm-before-execute;
- harden any local server (MCP or llama-server) against DNS rebinding and other local processes.

### Cited Findings
- MongoDB Compass sends prompt text and schema details to Microsoft/OpenAI (stating the data isn't stored or used for training). — [MongoDB docs](https://www.mongodb.com/docs/compass/query-with-natural-language/)
- Redis Copilot may send indexing schemas and sample keys to the Redis Copilot backend and OpenAI. — [Redis Copilot FAQ](https://redis.io/docs/latest/develop/tools/insight/copilot-faq/)
- Bruno AI routes no data through Bruno's servers (BYO keys). Raycast local models send nothing to Raycast or third parties. — [EveryDev: Bruno AI](https://www.everydev.ai/tools/bruno-ai); [Raycast manual](https://manual.raycast.com/ai/local-models)
- **Indirect prompt injection** (OWASP LLM01:2025) means malicious instructions embedded in data the LLM processes, such as web pages, documents or retrieved content. — [Oligo Security](https://www.oligo.security/academy/owasp-top-10-llm-updated-2025-examples-and-mitigation-strategies); [FutureAGI](https://futureagi.com/blog/owasp-llm-top-10-2025-risks-mitigations-2026/)
- **Tool-layer mitigations** (secondary summary of OWASP guidance):
  - One narrow tool per action with minimal permissions.
  - Validate tool inputs, because model tool calls are untrusted.
  - Return only needed fields.
  - Require explicit confirmation for destructive actions.
  - Log every tool call.
  - Keep secrets out of system prompts.
  - Regex input scanning is only a first layer.
  
  — [dev.to: OWASP LLM Top 10](https://dev.to/amasen/owasp-llm-top-10-what-every-engineer-building-with-ai-needs-to-know-in-2025-2gp8)
- **Scoping patterns in infra tools:** Lens Prism runs within RBAC and the user's kubeconfig; K8sGPT is read-only by design; kubernetes-mcp-server offers `--read-only`/`--disable-destructive` and recommends a view-only ServiceAccount; Postman's cloud agent runs in an isolated sandbox with a full audit trail. — [Lens blog](https://k8slens.dev/blog/introducing-lens-prism); [K8sGPT](https://github.com/k8sgpt-ai/k8sgpt); [kubernetes-mcp-server](https://glama.ai/mcp/servers/@containers/kubernetes-mcp-server); [Postman Agent Mode Cloud](https://learning.postman.com/docs/postman-ai/agent-mode/agent-mode-cloud/overview)
- **Enterprise kill switch:** Insomnia Enterprise admins can disable AI instance-wide. — [Insomnia docs](https://developer.konghq.com/insomnia/ai-in-insomnia/)
- **Local server hardening:**
  - MCP Streamable HTTP MUST validate Origin and SHOULD bind 127.0.0.1 and authenticate.
  - Local MCP servers should prefer stdio or require a token / IPC.
  - Real DNS-rebinding CVEs hit MCP SDKs in 2025.
  - Tauri sidecar capabilities can allowlist spawn arguments with regex validators, which limits what the webview can launch.
  
  — [MCP Streamable HTTP](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http); [MCP Security Best Practices](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices); [NVD CVE-2025-66416](https://nvd.nist.gov/vuln/detail/CVE-2025-66416); [Tauri sidecar docs](https://v2.tauri.app/develop/sidecar/)
- The MCP security guide also warns about **SSRF** when clients fetch URLs supplied by servers (block private ranges and 169.254.169.254, validate redirects). This matters because /dev/all's HTTP client *intentionally* hits localhost and private IPs. — [MCP Security Best Practices](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices)

### Inferences
- **Redaction pipeline before any remote provider call:**
  - Mask values of known-secret fields: Authorization headers, cookies, `*_token`, `*_key`, `password`, environment variables marked secret.
  - Detect high-entropy strings and JWTs (the app already has a JWT decoder).
  - Optionally run a small local NER/PII classifier (e.g., an ort-based model) on response bodies and logs.
  - Show a "what will be sent" preview. Default to *local-only* for anything touching DB rows or logs; *remote allowed* should be an explicit per-project setting.
- **Prompt-injection posture:**
  - Wrap untrusted content in delimiters, and tell the model it is data.
  - Never let a model that has read untrusted content trigger side-effecting tools without user confirmation. This is the "rule of two": untrusted input + tool access + sensitive data shouldn't coexist without a human gate.
  - Render AI output as text, never as HTML/JS in the webview (XSS → Tauri IPC escalation is the stdio-proxy attack class in the MCP guide).
- **Agentic action tiers in the UI:**
  - Read-only: no prompt.
  - Local side effects: one-click confirm with a diff or preview.
  - External or destructive actions (DB writes, kubectl apply, public tunnel, load test above N RPS): typed confirmation, plus an audit log stored in the project.
- **AI-initiated HTTP requests from the HTTP client** should get an SSRF guard by default (block cloud metadata IPs), which the user can override for their own endpoints.
- The managed llama-server sidecar should bind 127.0.0.1 on a random port with `--api-key` set. It is otherwise reachable by any local process or DNS rebinding (by analogy with the MCP CVEs; no specific llama-server CVE was searched).

### Gaps
- No first-party OWASP page was fetched (only secondary summaries).
- No data on real prompt-injection incidents via API responses or logs in dev tools.
- No existing Rust redaction/PII crates were evaluated.
- Whether llama-server's `--api-key` and CORS defaults are safe was not verified.

---

## 7. High-value and novel AI features that use "everything in one project"

### Takeaway
Competitors each see one slice (requests, or DB, or cluster). /dev/all's edge is cross-tool context: captured traffic, mocks, webhooks, load-test results, logs, DB schemas and K8s state in one project. The best features pair **deterministic extraction** (schema inference, diffing, stats, analyzers) with **small-model, schema-constrained generation and explanation**. That combination works well with 4–9B local models.

### Cited Findings (existing implementations that validate demand or approach)
- AI mock generation from a prompt, an OpenAPI spec, or a captured live URL response already exists (Insomnia 12). — [Insomnia docs](https://developer.konghq.com/insomnia/ai-in-insomnia/)
- Turning a collection of requests into MCP tools, one tool per request (Postman MCP Generator). — [Postman docs](https://learning.postman.com/docs/postman-ai/mcp-servers/generate/)
- Test-script and pre-request-script generation, request-body generation and request naming (Hoppscotch); test and docs generation (Bruno AI). — [Hoppscotch docs](https://docs.hoppscotch.io/documentation/features/ai-features); [EveryDev: Bruno AI](https://www.everydev.ai/tools/bruno-ai)
- NL → Mongo filters and aggregations (Compass), NL → Redis search queries (Redis Copilot), NL → SQL with schema context (DataGrip, DBeaver). — [MongoDB docs](https://www.mongodb.com/docs/compass/query-with-natural-language/); [Redis FAQ](https://redis.io/docs/latest/develop/tools/insight/copilot-faq/); [DataGrip](https://jetbrains.com/datagrip/features/ai); [DBeaver blog](https://dbeaver.com/2026/01/22/ai-for-sql-coding-in-dbeaver/)
- Deterministic analyzers plus LLM explanation for cluster problems (K8sGPT). The LLM runs kubectl within RBAC (Lens Prism). — [K8sGPT](https://k8sgpt.ai/docs/tutorials/ollama); [Lens](https://k8slens.dev/blog/introducing-lens-prism)
- AI commit-message and grouping suggestions for git-synced collections (Insomnia). — [Insomnia docs](https://developer.konghq.com/insomnia/ai-in-insomnia/)
- Comparing AI model responses, latency and token counts, and comparing MCP servers inside an API client (Postman). — [Postman AI overview](https://learning.postman.com/docs/postman-ai/overview)
- Classification and scoring as a first-class local-model use: Ollama's "decision models" return "choices, probabilities, and scores instead of text" (Sept–Oct 2026 release notes; unverified, see §3). — [Ollama releases](https://github.com/ollama/ollama/releases)
- Embedding models with explicit code-retrieval modes exist at about 300M parameters, small enough to embed in-app. — [HF: EmbeddingGemma](https://huggingface.co/google/embeddinggemma-300m)

### Inferences (feature ideas for /dev/all; mostly not seen shipped by competitors)
1. **Traffic → OpenAPI** ("reverse-engineer spec").
   - Deterministically cluster captured requests (HTTP client history, proxy/tunnel, webhook inbox) by method and path template, and infer JSON schemas by merging samples.
   - Then use the LLM only to name path parameters and operations, write descriptions, and spot enums or formats.
   - Output OpenAPI 3.1 constrained by a JSON schema.
2. **Traffic → mocks with realistic data.** From the inferred schema plus sampled values, generate a mock server: stateful CRUD where obvious (e.g., POST /users then GET /users/{id}), plus error cases seen in traffic (4xx/5xx). The LLM fills realistic, locale-aware data via schema-constrained output. This extends Insomnia's single-response mock idea to whole traffic captures.
3. **Smart mock data generator.** Use the DB schema (from the DB client) or an OpenAPI schema to produce referentially consistent fixtures (foreign keys honoured), exported as SQL inserts, JSON or CSV. The LLM picks semantic generators per column (email, IBAN-like test format, address), and a deterministic faker produces the volume. This is faster and more reproducible than LLM-generated rows.
4. **"Why did this request fail?" diagnosis.** Combine the request, response, timing, the matching mock or spec, recent server logs from the log viewer and K8s pod events. Run deterministic checks first (status code, schema diff against spec, auth header expiry via the JWT decoder, DNS/TLS errors), then have the LLM explain and propose a fix as an *editable diff* to the request.
5. **Load-test explainer.** Compute stats deterministically (latency percentiles over time, error-rate inflection, throughput plateau, correlation with pod CPU or restarts if K8s is connected). The LLM writes a short narrative and likely bottleneck hypotheses. Small-model-friendly because the facts are precomputed.
6. **NL → X translators with live validation:**
   - regex, validated against sample strings in the regex tool;
   - SQL, validated with EXPLAIN in the DB client, read-only by default;
   - jq/JSONPath, run against the current response;
   - kubectl, dry-run or server-side validation;
   - cron, date formats, curl ↔ request conversion.
   
   Each output is grammar-constrained where possible, plus a "validate before accept" step (the Hoppscotch/Compass pattern).
7. **Auto-generate test assertions from history.** From N successful responses, infer invariants (stable fields, types, ranges, header presence). Have the LLM turn them into readable assertions, then flag flaky fields (timestamps, IDs) deterministically.
8. **Webhook payload summariser and router.** Summarise incoming webhook bursts ("37 Stripe events, 3 failed payment_intent…"), detect the event type and provider, and generate a handler stub or a mock replay script.
9. **PII and secret detection in logs, responses and captured traffic.** A local classifier plus regex/entropy flags PII before sharing, exporting or sending to cloud AI. It doubles as the redaction engine (§6) and is a selling point for regulated users.
10. **Project-wide semantic search.** Run a local embedding index (EmbeddingGemma/Qwen3-Embedding/nomic via ort or llama-server `/v1/embeddings`) over requests, responses, specs, mocks, logs, saved queries and notes, e.g. "where did we call the refunds endpoint with a 409?". The index stays in the project's local store.
11. **Spec drift and contract checks.** Diff live traffic against the OpenAPI spec and mocks deterministically. The LLM explains breaking changes and drafts changelog notes.
12. **MCP exposure** of all the above as tools, so external agents (Claude Code) can, for example, "generate a mock from yesterday's traffic and point my tests at it". Optionally offer an **MCP client and inspector** for testing users' own MCP servers (a demand signal from Yaak).
13. **Model and prompt playground for API developers.** Postman-style side-by-side comparison of LLM endpoints (latency, tokens, cost) inside the HTTP client. This is cheap to add once the provider abstraction exists.
- **Prioritisation suggestion** (inference): ship 6 (NL→X with validation), 3 (mock data), 4 (failure diagnosis) and 12 (MCP) first. They work with 4–9B local models and have clear competitor precedent. Then build 1, 2 and 11 (traffic→spec/mocks/drift) as the differentiators. 5, 7, 8, 9 and 10 build on the same infrastructure.

### Gaps
- No user research or usage data on which AI features developers actually use in API clients (no HN/Reddit or survey data found).
- I found no existing product doing full traffic → OpenAPI inference with AI in a desktop API client. Absence isn't proof: the search budget ran out before checking tools like Optic, Akita and HTTP Toolkit.
- No measured accuracy of 4–9B models on NL→SQL/regex/jq tasks specifically.

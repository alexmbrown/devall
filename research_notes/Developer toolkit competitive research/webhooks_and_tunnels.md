# Webhook Testers, Localhost Tunnels, and Relay Infrastructure (survey as of Oct 2026)

All sources were accessed on 2026-10-21 unless noted. Prices come from live pricing pages on that date; most of those pages show no effective date, so treat them as "as listed Oct 2026". Anything published before 2025 is marked **[pre-2025, may be outdated]**. Where a figure came from a third-party aggregator rather than the vendor, it is labelled.

---

## 1. Webhook testers: features, limits, pricing (webhook.site, Hookdeck, smee.io, RequestBin/Pipedream, Beeceptor, Svix Play, Webhook Relay, ngrok inspector, Postman/Apidog) and provider signature schemes

### Takeaway
Webhook testers fall into three groups. **Capture-and-inspect bins** (webhook.site, Beeceptor, Pipedream) are generous enough for casual use but cap the free tier hard: 100 requests per URL with a 7-day expiry on webhook.site, 50 requests/day on Beeceptor. **Relays that forward to localhost** (Hookdeck CLI/Console, smee.io, Svix Play, webhook.site CLI) are free and need no account, but either store nothing (smee) or keep data only briefly. **Full gateways** (Hookdeck Event Gateway, Webhook Relay) charge per event. The gap for /dev/all is a free, persistent, per-project URL with local-first storage, replay, and signature-aware tooling. No surveyed tool combines all of these for free.

### Cited Findings

**webhook.site**
- Free URLs stop accepting requests after 100 (then return HTTP 429), expire after 7 days with data deleted, and are readable by anyone who knows the URL ID. Paid URLs have no incoming cap, keep the latest 10,000 requests by default, don't expire, purge data after at most 365 days, and are login-protected — [webhook.site docs](https://docs.webhook.site/)
- Request bodies must be under 10 MB (HTTP 413 above that). Paid URLs with "extraordinarily high traffic" may be blocked automatically under the ToS — [webhook.site docs](https://docs.webhook.site/)
- Features include Custom Actions (scripting/"WebhookScript"), Modify Response, Send Email, Schedules, CSV export, custom aliases, DNSHook and email capture, CLI forwarding to a local URL, and "XHR Redirect" through an open browser tab (the target needs CORS). Custom domains are Enterprise only — [webhook.site docs](https://docs.webhook.site/)
- Paid plans: **Basic $9/mo ($90/yr)**, **Pro $19/mo ($190/yr)**, **Enterprise $69/mo ($699/yr)**, USD excluding VAT. URLs per plan: 1 / 50 / unlimited. Request history per URL: 1,000 / 10,000 / 100,000. 365-day storage. CLI localhost forwarding on Basic and above. Custom domain and SAML SSO on Enterprise only — [webhook.site Basic, Pro and Enterprise](https://docs.webhook.site/pro.html). Conflict: a search snippet of the same docs page showed $9/$18/$59, and a third-party review (Gappsy) lists Pro at $27/mo. The fetched docs page ($9/$19/$69) is treated as authoritative — [Gappsy review](https://gappsy.com/tools/webhook-site/)

**Hookdeck (Console, CLI, Event Gateway)**
- **Console** (console.hookdeck.com): free and "needs no account". Each source gets an ingest URL (`console.hookdeck.com/e/<id>`) and an inspect URL. It shows method, headers, path, query, and body, and supports replay and forwarding to localhost (via the CLI) or any HTTP endpoint. Without an account, sources are kept in the browser for "up to 90 days". A Webhooks Library sends sample provider events, and a Console API can create sources with one POST and no API key — [Hookdeck Console docs](https://hookdeck.com/docs/console)
- **CLI**: described as a free alternative to ngrok. Login is optional, and a guest account is created automatically. `hookdeck listen <port>` forwards to `localhost:$PORT/$DESTINATION_PATH`. Interactive mode browses up to 1,000 events, shows full request/response, and supports retry/replay (`r` key). Events are visible and filterable on the dashboard — [Hookdeck CLI docs](https://hookdeck.com/docs/cli)
- **Event Gateway pricing**: Developer $0 (10,000 events, 3-day retention, 1 user). Team from $39/mo (7-day retention). Growth from $499/mo (30-day retention, 99.999% uptime SLA). Enterprise custom. Overage is $3.00 per 100k events up to 5M, falling to $0.35 per 100k above 500M. 5 events/sec per destination is included, and extra throughput costs $3 per event/sec (6–25). Excess events are queued, not dropped. Unlimited connections and transformations on all plans. 100k discarded requests/month are free, then $2.50/M. Static IP add-on $100 — [Hookdeck pricing](https://hookdeck.com/pricing)

**smee.io (Probot)**
- Pass-through relay that sends payloads to connected clients over Server-Sent Events. Payloads "are not stored on the server or in any database", and the viewer keeps recent ones in browser `localStorage`. Channels are unauthenticated: anyone with the channel ID can see payloads, and it's positioned as dev/test only — [probot/smee.io GitHub](https://github.com/probot/smee.io)
- MIT-licensed Node.js app, self-hostable via Docker image `ghcr.io/probot/smee.io`, with Redis (`REDIS_URL`) as a message bus for multi-instance deployments. About 485 stars, 13 open issues, 29 open PRs, not archived — [probot/smee.io GitHub](https://github.com/probot/smee.io)

**Svix Play**
- "Free forever, no signup required". Gives a unique, persistent HTTPS URL, shows body, headers, and signature in the browser, and has a Play API to query webhook history from tests. `svix listen` relays to a local server. No limits, retention, or custom-response features are documented — [Svix Play](https://www.svix.com/play/)
- The Svix Play CLI is listed as MIT-licensed and "for development, not production" — [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling)

**Beeceptor**
- Free $0: 50 requests/day per endpoint, 3 mock rules, public endpoints. Unclaimed endpoints are deleted 30 days after creation; claiming one (free account) extends this to 90 days. Includes dynamic/conditional responses, live inspection, and an *ephemeral* local tunnel — [Beeceptor pricing](https://beeceptor.com/pricing/)
- Individual $10/mo (15k req/month, 50 rules, private). Team $25 (100k/month, 250 rules, custom domains, mTLS). Scale $99 (1M+/month, $40 per extra 1M, forward proxy, historical inspection). Persistent local tunnel on all paid plans. 20% off annually. No refunds — [Beeceptor pricing](https://beeceptor.com/pricing/)

**Webhook Relay**
- Free: 150 webhooks/month, 1 input, 2 outputs, **0 bidirectional tunnels**. Buckets are suspended when the limit is hit. Basic $8.99/mo (5,000 webhooks, 8 tunnels, $5 per extra 1k). Business $71.99/mo (60k, 20 tunnels). Pro $224.99/mo (1M, 50 tunnels). 10% off yearly. Every received webhook counts, including failures, and fan-out to 3 destinations counts as 3. 40 MB webhooks, custom domains, and static outgoing IP are listed on all plans. An agent is needed only for internal/localhost destinations — [Webhook Relay pricing](https://webhookrelay.com/pricing/)

**Pipedream (HTTP triggers / RequestBin lineage)**
- Each HTTP trigger gets a unique `*.m.pipedream.net` URL (custom domains supported). By default anyone with the URL can trigger it, with optional Bearer token or OAuth. The default response is `200 OK` "Success!". `$.respond()` sets a custom status, headers, and body, and `immediate: true` responds mid-workflow — [Pipedream triggers docs](https://pipedream.com/docs/workflows/building-workflows/triggers)
- Default body limit is 512 KB (413 above). The `x-pd-upload-body: 1` header allows payloads of any size via S3 (signed URL valid 30 min). Free/Basic plans keep at most 100 events or 7 days per workflow. Undocumented 429 rate limits — [Pipedream triggers docs](https://pipedream.com/docs/workflows/building-workflows/triggers)

**ngrok Traffic Inspector (as a webhook tester)**
- The cloud Traffic Inspector stores metadata by default. "Full Capture" (opt-in by an admin) stores headers and bodies up to **10 KB per request/response** (header values truncated at 1 KB). Retention: Free 24 h, Hobbyist/PAYG 72 h, add-on up to 90 days — [ngrok Traffic Inspection docs](https://ngrok.com/docs/obs/traffic-inspection)
- Replay requires Full Capture. It resends from a different IP, strips `Connection: Keep-Alive`, and adds `Ngrok-Replay-Original-Request-ID`. "Replay with modifications" lets you edit before resending (same-account endpoints only, no forbidden headers). Truncated or oversized requests can't be replayed. The local agent inspector runs at `http://localhost:4040` — [ngrok Traffic Inspection docs](https://ngrok.com/docs/obs/traffic-inspection)
- The free plan includes 500 "webhook verifications"/month (ngrok's Traffic Policy webhook-verification action) — [ngrok free plan limits](https://ngrok.com/docs/pricing-limits/free-plan-limits)

**Postman**
- Postman "collection webhooks" are the reverse of a tester. You create them via the Postman API (`POST /webhooks`), and a POST to the webhook URL triggers a monitor-run of a collection with the JSON body in the `previousRequest` global. Output is visible only in monitor console logs. The docs don't describe capturing or inspecting arbitrary incoming webhooks — [Postman collection webhooks](https://learning.postman.com/docs/collections/running-collections/collection-webhooks/)

**Provider signature schemes (what a "signature verification helper" must implement)**
- **Stripe**: header `Stripe-Signature: t=<ts>,v1=<sig>[,v0=<test sig>]`. `signed_payload = timestamp + "." + raw body`, HMAC-SHA256 keyed with the endpoint secret (`whsec_…`). Only `v1` is valid (ignore other schemes to avoid downgrade). Several `v1` signatures can appear during secret rotation (up to 24 h). Libraries default to a **5-minute tolerance**, and each retry gets a new timestamp and signature. `stripe listen --forward-to localhost:4242/webhook` prints a CLI-specific `whsec_` that differs from the Dashboard secret — [Stripe: Receive events](https://docs.stripe.com/webhooks); [Stripe: manage endpoints / signature errors](https://docs.stripe.com/webhooks/signature)
- **GitHub**: `X-Hub-Signature-256: sha256=<hex HMAC-SHA256(secret, raw body)>`. Legacy `X-Hub-Signature` uses HMAC-SHA1. Compare in constant time. Test vector: secret `It's a Secret to Everybody`, payload `Hello, World!` → `757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17` — [GitHub: validating deliveries](https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries)
- **Shopify**: `X-Shopify-Hmac-SHA256` = base64(HMAC-SHA256(app client secret, raw body)). Shopify allows a 1 s connect timeout and a 5 s total request time, then retries 8 times over 4 hours. Admin-API subscriptions are auto-deleted after 8 consecutive failures. `X-Shopify-Webhook-Id` is used for dedup and `X-Shopify-Event-Id` for correlation. A new secret can take up to an hour to take effect after rotation — [Shopify HTTPS webhooks](https://shopify.dev/docs/apps/build/webhooks/subscribe/https)
- **Twilio**: `X-Twilio-Signature` = HMAC-SHA1 keyed with the account auth token over the **full URL Twilio called** plus alphabetically sorted POST params appended. JSON bodies carry a `bodySHA256` query param. "If you decode or re-encode the URL, signature validation fails" — [Twilio webhook security](https://www.twilio.com/docs/usage/webhooks/webhooks-security)
- **Slack**: `X-Slack-Signature: v0=<hex HMAC-SHA256(signing secret, "v0:" + X-Slack-Request-Timestamp + ":" + raw body)>`. Reject requests more than 5 minutes old. Compare in constant time — [Slack: verifying requests](https://docs.slack.dev/authentication/verifying-requests-from-slack)
- **Standard Webhooks** (Svix-led spec, Apache-2.0, v1.0.0): headers `webhook-id`, `webhook-timestamp`, and `webhook-signature` (space-separated `v1,<base64>` entries). Signs `msg_id.timestamp.payload` with HMAC-SHA256. The secret is `whsec_` + base64 of 24–64 random bytes. `v1a` is ed25519 (`whsk_`/`whpk_` keys) — [Standard Webhooks spec](https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md)

### Inferences
- **Byte-exact forwarding is mandatory.** All five providers sign the raw body, and Twilio also signs the exact public URL. The relay and desktop forwarder must preserve raw bytes and expose the original public URL/Host (e.g., `X-Forwarded-Host`/`X-Original-URL`). Otherwise users' local Twilio validation will fail behind the tunnel.
- **Naive replay breaks timestamped signatures.** Replaying Stripe or Slack events more than 5 minutes later fails tolerance checks. A delight feature would be "replay with re-sign": the user supplies a test secret, and /dev/all regenerates `Stripe-Signature`/`X-Slack-Signature`/Standard Webhooks headers with a fresh timestamp. Pair it with a "verify signature" panel that shows the computed vs received HMAC.
- **Retention is cheap for /dev/all.** Free tiers are cheap for vendors because data is purged in 1–7 days or capped at 100 requests. A local-first app can keep the relay's retention short (a buffer only) while keeping full history on disk, which beats every surveyed free tier on retention at near-zero server cost.
- **Respond fast.** Shopify's 5 s total timeout means the relay must ack quickly when the desktop is offline (store-and-forward, Hookdeck-style), not block on a local round-trip, at least in "capture" mode.

### Gaps
- Apidog's webhook/mock features were not researched (no source fetched).
- RequestBin.com's current status (standalone vs folded into Pipedream) could not be confirmed. The Pipedream page fetched didn't mention it.
- Hookdeck CLI license, Hookdeck free-plan rate limits, and Svix Play retention/limits are undocumented on the pages fetched.
- webhook.site free-tier API rate limits and WebhookScript capabilities per tier weren't specified.

---

## 2. Tunnel services: features, protocols, custom domains, auth, inspection/replay, pricing, open-source status

### Takeaway
ngrok remains the reference product (inspection, replay, OAuth/OIDC, Traffic Policy), but its free tier is quota-bound: 1 GB, 20k requests/month, a browser interstitial, and one assigned dev domain. Cloudflare Tunnel is the free power-user default, though Quick Tunnels are random-URL, no-SSE, and best-effort. Tailscale Funnel, Microsoft Dev Tunnels, Pinggy, zrok, LocalXpose, Serveo, and Expose fill niches; most gate stable subdomains or custom domains behind $6–$20/mo. Interstitial warning pages on free tiers are now industry-standard (ngrok, zrok, Serveo, Dev Tunnels).

### Cited Findings

**ngrok (pricing page, Oct 2026; no effective date shown)**
- **Free $0** includes a one-time $5 usage credit (valid one year): 3 online endpoints, 3 concurrent agents, 1 GB data out, 20k HTTP requests/month, 5k TCP connections/month, 4k HTTP req/min, 100 TCP conn/min, 1 dev domain, **interstitial page: yes**, Traffic Inspector 24 h, 3 OAuth/OIDC "traffic identity" MAU, HTTP and TCP (TCP addresses are random and need card verification) — [ngrok pricing](https://ngrok.com/pricing)
- **Hobbyist $10/mo** ("save $24 with annual"): 3 endpoints, 5 GB then $0.10/GB, 100k requests then $1 per 100k, 20k req/min, 10 ngrok-branded domains, **no interstitial**, 72 h inspector, 5 MAU identities — [ngrok pricing](https://ngrok.com/pricing)
- **Pay-as-you-go $20/mo + usage**: unlimited endpoints and agents. Bring-your-own domain at $0.01 per active hour, TLS endpoints, 3 users then $5/user, and active endpoint hours at $0.02/h (only hours with traffic count). Add-ons: SSO/RBAC $10/user/mo, IP restrictions/SCIM/audit suite $15/user/mo, dedicated Agent Connect IPs $900/mo/region — [ngrok pricing](https://ngrok.com/pricing)
- The official free-plan doc says there is **no session timeout** ("Free endpoints can stay online indefinitely"). It also lists 10,000 logs & events, 500 webhook verifications, 5 Traffic Policy rules per policy, and no raw TLS endpoints. The free domain is auto-assigned (e.g., `your-assigned-name.ngrok-free.app`), and custom/reserved/random/wildcard domains need a paid plan — [ngrok free plan limits](https://ngrok.com/docs/pricing-limits/free-plan-limits). Conflict: third-party remote.it and livemy.app claim a 2-hour session cap and forced random URLs on free accounts. These claims are unverified and contradicted by the vendor doc — [remote.it vs ngrok](https://www.remote.it/resources/remote-it-vs-ngrok); [livemy.app](https://livemy.app/blog/ngrok-alternatives)
- **2026 pricing changes** (blog dated 13 Jul 2026): Traffic Policy units cut from $1 to $0.10 per 100k (effective 22 Jun 2026). Custom domains are billed $0.01/h only for hours with traffic (from 29 Jun 2026; previously billed from the moment a domain was added). Endpoint pools are now billed per active endpoint — [ngrok blog: pricing June 2026](https://ngrok.com/blog/pricing-june-2026)
- Interstitial details: shown before HTML browser traffic on free. Clicking "Visit" sets a cookie for 7 days. It doesn't affect API/programmatic clients and is bypassed by the `ngrok-skip-browser-warning` header or a non-standard User-Agent — [ngrok free plan limits](https://ngrok.com/docs/pricing-limits/free-plan-limits)
- ngrok ships an embeddable Rust SDK (`ngrok` crate, MIT/Apache-2.0), "the ngrok agent packaged as a Rust crate", with tokio AsyncRead/AsyncWrite connections and `listen_and_forward`. Its quickstart pins 0.14.0, it authenticates with an authtoken, and the repo contains a `muxado` directory (ngrok's stream multiplexer) — [ngrok/ngrok-rust](https://github.com/ngrok/ngrok-rust)

**Cloudflare Tunnel / Quick Tunnels (trycloudflare.com)**
- Quick Tunnels: no Cloudflare account or domain needed. The hostname changes every run, and the URL dies when `cloudflared` stops. Hard cap of **200 in-flight requests** (429 beyond). **No Server-Sent Events.** "No uptime guarantee", dev/test only — [Cloudflare Quick Tunnels docs](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/)
- Named tunnels: `cloudflared` makes outbound-only connections to Cloudflare's edge, and a tunnel is a persistent UUID object with multiple connectors. It can front HTTP servers, SSH, RDP, and "other protocols" — [Cloudflare Tunnel docs](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/)
- Transport: `--protocol auto` (default) tries **QUIC** first and falls back to **HTTP/2** if UDP is blocked. QUIC uses post-quantum key agreement by default — [cloudflared run parameters](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/cloudflared-parameters/run-parameters/)
- Account limits: 1,000 tunnels per account, 25 active replicas per tunnel, 50,000 routes. "Tunnels for Platforms" handles more than 1,000 tunnels — [Cloudflare One account limits](https://developers.cloudflare.com/cloudflare-one/account-limits/)
- The `cloudflared` client is Apache-2.0 (Go). The awesome-tunneling maintainer calls Cloudflare Tunnel free, "production-quality", and his top recommendation for most people — [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling). (Cloudflare's own Tunnel pages fetched did not state a price.)

**Tailscale Funnel**
- Beta (doc validated 20 Jan 2026). Ports **443, 8443, 10000 only**, TLS only, URLs on the tailnet's `ts.net` domain only (no custom domain documented). Bandwidth limits exist but are unpublished and not configurable. Available on all plans. Requires v1.38.3+, MagicDNS, HTTPS certs, and a `funnel` node attribute in the tailnet policy — [Tailscale Funnel docs](https://tailscale.com/kb/1223/funnel)
- Architecture: public DNS resolves to a Funnel relay that TCP-proxies to the device over Tailscale. **The relay cannot decrypt traffic**, because TLS terminates on the device. New DNS records can take up to 10 minutes to propagate — [Tailscale Funnel docs](https://tailscale.com/kb/1223/funnel)

**Microsoft Dev Tunnels / VS Code port forwarding**
- Public preview, "adhoc testing and development, not for production". Private by default (Microsoft, Entra ID, or GitHub account required to access). Persistent URLs, multiple ports per tunnel, and traffic inspection in browser DevTools — [Microsoft Dev Tunnels overview](https://learn.microsoft.com/en-us/azure/developer/dev-tunnels/overview) (updated 12 Feb 2026)
- Limits (via third-party summary of Microsoft's limits page): 5 GB/user/month, 10 tunnels/user, 10 ports/tunnel, 1,000 active connections/port, 1,500 HTTP req/min/port, 20 MB/s/tunnel, 16 MB max request body. URLs are `*.devtunnels.ms` with no custom domain. Unused tunnels are deleted after 30 days — [merginit 2026 comparison](https://merginit.com/blog/19062026-free-developer-tunnels-comparison) (Microsoft's own limits page URL returned 404 to me)
- Dev Tunnels "shows warnings on visited URLs" and has no custom domains — [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling)

**zrok (NetFoundry / OpenZiti)**
- Free: 5 GB/day bandwidth, 25 environments, 50 shares (ephemeral or reserved), 50 private access frontends, community support. Unverified free accounts get an anti-phishing interstitial, removed by adding a card. Commercial plan is contact-sales. **Self-host: no limits, Apache-2.0** — [zrok pricing](https://zrok.io/pricing/)
- Features: public and private shares, private frontends for HTTP/TCP/UDP/VPN, file/drive sharing, reserved names, automatic TLS, Google/GitHub OAuth on public shares, and large-payload/SQLi defenses — [zrok pricing](https://zrok.io/pricing/)

**Pinggy**
- Free: 60-minute tunnel timeout, random subdomains, unlimited data. Protocols: HTTP(S), TCP, UDP, TLS. Works with plain `ssh` ("Without downloading any binary!"). The web debugger can inspect, modify, and replay, and is included on free. Basic/bearer auth and IP whitelisting are available. A desktop app exists. Custom domains and persistent subdomains are Pro only. The Pro price isn't rendered on the page ("save up to 17%" yearly) — [Pinggy](https://pinggy.io/)

**localtunnel**
- MIT, Node.js, 22.5k stars, 151 open issues, 17 open PRs. `lt --port` / `npx localtunnel`, `--subdomain` requests, auto-reconnect, and a Node API. The default server is localtunnel.me (server code in a separate repo) — [localtunnel GitHub](https://github.com/localtunnel/localtunnel)
- HN commenters report reliability issues with localtunnel — [HN: rustunnel thread](https://news.ycombinator.com/item?id=47425918)

**bore, frp, rathole** (self-hosted OSS; details in Q3)
- bore: MIT, Rust, TCP only, public server `bore.pub` — [ekzhang/bore](https://github.com/ekzhang/bore)
- frp: Apache-2.0, Go, TCP/UDP/HTTP/HTTPS/STCP/XTCP (P2P)/TCPMUX, dashboard, OIDC auth, ~110k stars — [fatedier/frp](https://github.com/fatedier/frp)
- rathole: Apache-2.0, Rust, TCP/UDP services over TCP/TLS/Noise/WebSocket, 14.4k stars — [rapiz1/rathole](https://github.com/rapiz1/rathole)

**Expose (Beyond Code)**
- Hobby free (time limit, random URLs, single EU server). **Pro $79/user/year**, **Team $229/team/year** (up to 10 users). Pro adds custom domains/subdomains, reserved subdomains, persistent URLs, access control, and a global network. Written in PHP; the open-source core is self-hostable. The site mentions receiving webhooks locally and replaying them — [expose.dev](https://expose.dev/)

**LocalXpose**
- Starter $0: 2 active HTTP/HTTPS tunnels with time limits. **PRO $8/mo or $96/yr**: 10 tunnels, HTTP/HTTPS/TCP/TLS/UDP, custom and wildcard domains, "unlimited" bandwidth (AUP). Basic auth, key auth, rate limiting, IP whitelisting, header editing, and a built-in file server on all plans — [LocalXpose pricing](https://localxpose.io/pricing)

**localhost.run and Serveo (SSH-based)**
- localhost.run: free with no signup, speed-limited. Free hostnames rotate after a few hours. HTTPS over SSH with optional TLS passthrough. The docs suggest `ServerAliveInterval`/`autossh` against drops — [merginit 2026 comparison](https://merginit.com/blog/19062026-free-developer-tunnels-comparison) (localhost.run's own docs failed to load with a TLS error)
- Serveo: `ssh -R 80:localhost:3000 serveo.net`, no client download, WireGuard option, beta Chrome extension. Free: 3 tunnels, custom subdomains, **interstitial warning**. Pro: $6 per 10-day pass, $6/mo, or $60/yr (unlimited tunnels, no interstitial). Running since 2017 — [serveo.net](https://serveo.net/)

**inlets**
- Commercial subscription: Personal $25/mo (up to 5 tunnels, personal use), Pro Commercial $50/mo (2 tunnels + $25 each), Uplink $250/mo (10 per cluster). HTTP tunnels carry HTTP/REST/gRPC/WebSockets/SSE, and TCP tunnels carry TLS, databases, SSH, etc. Inlets Cloud provides managed exit servers — [inlets pricing](https://inlets.dev/pricing/). Listed as "Formerly open source; now a commercial product" — [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling)

**Others noted in awesome-tunneling (commercial)**: Webhook Relay (bidirectional tunnels paid only), Beeceptor (local tunnel), playit.gg (Rust client open, server closed), LocalToNet (UDP), Horizon Tunnel, instatunnel, LocalCan (desktop app + custom domains), Lokal.so, and Openport.io — [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling)

### Inferences
- **Price anchors for a paid tier**: ~$6–10/mo for a stable subdomain, no interstitial, and longer retention (Serveo $6, LocalXpose $8, ngrok Hobbyist $10, Beeceptor $10); ~$20/mo for custom domains and TCP/TLS (ngrok PAYG).
- **A best-in-class /dev/all tunnel** should offer what nobody bundles for free: a stable per-project subdomain, HTTP+WebSocket+SSE (Quick Tunnels lack SSE), on-port-443 transport, local unlimited inspection/replay (ngrok caps capture at 10 KB and 24 h on free), and an interstitial only for HTML browser traffic, never for webhook/API clients.
- **ngrok-rust as fallback**: it is the only embeddable *commercial* tunnel SDK found in Rust. /dev/all could offer "bring your own ngrok token" as a fallback provider at no infra cost, alongside a native relay. Using Cloudflare quick tunnels via a spawned `cloudflared` is another zero-infra fallback (subject to Cloudflare's terms).

### Gaps
- Pinggy Pro price (not rendered on the page), localhost.run pricing (docs fetch failed), and Tailscale Funnel bandwidth caps (unpublished) are unknown.
- Microsoft's own Dev Tunnels limits page could not be fetched (404). Limits above come from a 2026 third-party summary.
- Cloudflare's own docs pages fetched did not state Tunnel pricing. "Free" relies on the awesome-tunneling maintainer and common knowledge.
- I found no official ngrok statement on whether usage via `ngrok-rust` counts against plan quotas (presumably it does, as it's the same agent).

---

## 3. Open-source tunnel implementations to embed or self-host (especially Rust), and their wire protocols

### Takeaway
Rust has mature, permissively licensed building blocks. **bore** (MIT, ~400 LoC, one TCP connection per proxied stream, no multiplexing, no encryption) is the simplest reference. **rathole** (Apache-2.0, TCP/TLS/Noise/WebSocket transports, ~500 KiB binary) is the most production-capable Rust option. Neither does HTTP host-based routing, inspection, or replay. The common designs in the wider ecosystem are: one control connection plus a data connection per stream (bore), a single multiplexed connection (yamux/muxado/HTTP/2), QUIC streams (cloudflared, reverst, tunelo, iroh), WebSocket-carried streams (rathole-ws, wstunnel, rustunnel, jprq), and plain SSH `-R` (Serveo, localhost.run, sish, Pinggy). For a relay hosted on Cloudflare Workers/DO or Cloud Run, **WebSocket is the only viable transport**. On a VPS or Fly.io, QUIC or TCP+yamux are open.

### Cited Findings
- **bore** (ekzhang): Rust, MIT, "about 400 lines of async Rust" on Tokio. Control port 7835: the client sends `Hello`, the server listens on a remote port. For each external connection the server sends a UUID, the client opens a **separate TCP stream** and sends `Accept(uuid)`, and the server splices them (unclaimed after 10 s are dropped). There is no multiplexing. An optional shared secret drives an HMAC challenge-response, but "traffic is not encrypted by default". TCP only, with no HTTP routing or TLS. ~11.5k stars. Public instance bore.pub. Published as the `bore-cli` crate (no documented library API) — [ekzhang/bore](https://github.com/ekzhang/bore)
- **rathole**: Rust, Apache-2.0. Transports TCP (default), TLS, **Noise protocol** (no certs needed), and **WebSocket (optionally TLS)**. Per-service tokens, app-layer heartbeats, UDP services, and hot reload of config (HTTP API "planned"). Claims "much higher throughput" than frp with lower memory (loopback benchmarks). Binary "can be as small as ~500KiB". 14.4k stars, 77 open issues. Release dates were not visible — [rapiz1/rathole](https://github.com/rapiz1/rathole)
- **frp**: Go, Apache-2.0, ~110k stars. Proxy types TCP/UDP/HTTP/HTTPS/STCP/XTCP (P2P)/TCPMUX. Transports: TCP stream multiplexing, KCP, QUIC, WSS. Dashboard, Prometheus, plugins, load-balancing groups, health checks, bandwidth limits, token or OIDC auth, TLS on by default. **frp v2 is in development and will be incompatible with v1**; the maintainer says progress is slow — [fatedier/frp](https://github.com/fatedier/frp)
- **zrok**: Go, Apache-2.0, built on the OpenZiti zero-trust overlay. Self-hosters can toggle interstitials per frontend/account — [zrok pricing](https://zrok.io/pricing/); [zrok blog](https://blog.openziti.io/zrok-is-growing-up)
- **ngrok-rust**: crate `ngrok`, MIT/Apache-2.0. Embeds the ngrok agent in-process (hyper-compatible listeners, tokio streams) and requires an ngrok account/authtoken. The repo includes `muxado` (ngrok's multiplexer) — [ngrok/ngrok-rust](https://github.com/ngrok/ngrok-rust)
- **cloudflared**: Apache-2.0 Go client. QUIC by default with HTTP/2 fallback — [cloudflared run parameters](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/cloudflared-parameters/run-parameters/); [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling)
- Other OSS options from awesome-tunneling (licenses and transports as listed there) — [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling):
  - **Rust**: tunnelto (MIT); wstunnel (proxies over WebSockets to bypass protocol blocks); rustunnel (AGPL-3.0, ngrok-style); tunelo (QUIC, self-hostable relay, MIT declared, LICENSE pending); Orbien (frp-like with web admin); YTunnel (Cloudflare Tunnel TUI); playit.gg client; iroh (MIT/Apache, QUIC peer connectivity by public key); Narrowlink (MPL/AGPL); EasyTier (LGPL)
  - **Go**: koding/tunnel (a library using **yamux**); go-http-tunnel ("single HTTP/2 connection"); reverst (Apache-2.0, **HTTP/3/QUIC** reverse tunnels, no releases since 2024); piko (MIT, production/Kubernetes, HTTP only per HN); portr (AGPL-3.0, admin page plus **request inspection/replay**); sish (SSH-based, request inspection); mmar (AGPL-3.0); jprq (WebSockets); Pangolin (recommended for production self-hosting)
  - **Other**: h2tunnel (MIT, Node, <500 lines, **HTTP/2 multiplexing**); OpenTunnel (TypeScript, "blind TLS on Cloudflare"); Tabserve.dev (Cloudflare Worker-based)
- rustunnel (HN, ~7 months old, 107 points) uses encrypted WebSocket connections and offers TLS termination, HTTP and TCP proxying, a dashboard, Prometheus metrics, and audit logs. A commenter pushed AGPL "to stop a proprietary hosted rebrand", and the author agreed — [HN: rustunnel](https://news.ycombinator.com/item?id=47425918)
- Piko is HTTP-only, runs as a fault-tolerant cluster, and is Kubernetes-friendly — [HN: Show HN Piko](https://news.ycombinator.com/item?id=40355744) **[pre-2025 thread, may be outdated]**

### Inferences
- **Recommended /dev/all client stack** (inference, not tested): Rust `tokio` with a WebSocket transport (e.g., tokio-tungstenite) to the relay on 443. Multiplex streams inside it with a small framing layer (stream-id, open/data/close/window), or run `yamux` over the WebSocket byte stream. This works through corporate proxies, which HN users raise as a pain point, and on Cloudflare DO and Cloud Run. Optionally add QUIC (`quinn`) when self-hosting on a VPS/Fly for lower head-of-line blocking. (Crate names are from general knowledge and weren't verified in this session.)
- **Fork or reimplement**: bore is the cleanest code to study or fork for a raw-TCP mode. rathole's transport abstraction (TCP/TLS/Noise/WS) is the best model for a pluggable transport layer, and its Apache-2.0 license permits embedding. Neither supports HTTP host routing, so the relay needs its own HTTP front (subdomain → session) plus capture/inspection.
- **Licensing**: if /dev/all open-sources its relay, the HN discussion suggests AGPL deters SaaS rebrands. MIT/Apache maximizes embedding. The desktop client embedding Apache/MIT crates (rathole, bore, ngrok-rust) has no copyleft issue.
- **Tailscale Funnel's model** (relay can't decrypt; TLS terminates on the device) is a privacy differentiator worth considering for "TLS passthrough" mode. But it prevents relay-side capture, so webhook inspection would then happen only locally.

### Gaps
- Latest release versions and dates for bore, rathole, frp, and ngrok-rust were not visible on the fetched GitHub pages, so maintenance recency is unconfirmed. rathole's maintenance status in 2026 especially needs checking: the README shows a 2026 copyright footer but no commit dates were visible.
- The wire protocols of tunnelto, localtunnel (believed to use a pool of raw TCP sockets), and ngrok's muxado framing were not confirmed from primary sources.

---

## 4. Infrastructure assessment: Firebase vs Cloud Run vs Cloudflare Workers+Durable Objects vs Fly.io vs VPS vs Supabase Realtime, and hobby-scale costs

### Takeaway
**Firebase can host the webhook capture endpoint, but not a usable reverse tunnel.** Cloud Functions 2nd gen HTTP requests max out at 60 minutes, Firebase Hosting rewrites cut requests at **60 seconds**, and nothing documents WebSocket support for Functions. Firestore/RTDB listeners can push captured webhooks to the desktop app (smee/Hookdeck-style store-and-forward), but synchronous request→localhost→response tunnelling would mean a function waiting on a database round-trip. Cloud Run supports WebSockets, but bills an instance as active for the whole time any socket is open and forces reconnects every ≤60 min. **Cloudflare Workers + Durable Objects with WebSocket Hibernation is the best fit for HTTP/WebSocket tunnels and webhook capture at ~$0–5/mo**, but can't accept inbound raw TCP. **A $3–7/mo Fly.io machine or VPS** is the right home for raw TCP/UDP/QUIC tunnels.

### Cited Findings

**Firebase (Cloud Functions for Firebase, Firestore, RTDB, Hosting)**
- Timeouts (not adjustable): 2nd gen HTTP functions **60 min**, 2nd gen event-driven 9 min, 1st gen 9 min. 2nd gen request body 32 MB, response 32 MB (non-streaming) or **10 MB for streaming responses**. 2nd gen networking limits defer to Cloud Run. Functions require the **Blaze** pay-as-you-go plan. The quotas page does not mention WebSockets or long-lived connections — [Firebase Functions quotas](https://firebase.google.com/docs/functions/quotas)
- **"Firebase Hosting is subject to a 60-second request timeout"**, so longer requests via Hosting rewrites return 504 even if the function allows more. pinTag relies on Cloud Run tags (1,000 per service) — [Firebase Hosting + Functions](https://firebase.google.com/docs/hosting/functions)
- Pricing — [Firebase pricing](https://firebase.google.com/pricing):
  - **Cloud Functions**: free 2M invocations, 400K GB-s, 200K CPU-s, and 5 GB egress per month; then $0.40/M invocations and $0.12/GB egress
  - **Firestore** (Standard): free 50K reads, 20K writes, and 20K deletes per day; 1 GiB stored; 10 GiB egress per month
  - **Realtime Database**: **100 simultaneous connections on Spark**, 200K per database on Blaze; 1 GB stored then $5/GB; 360 MB/day downloads then $1/GB
  - **Hosting**: 10 GB storage; 360 MB/day transfer then $0.15/GB
  - Eligible Blaze users get $300 credit

**Google Cloud Run (direct, without Firebase Hosting)**
- WebSockets are supported. The request timeout is "currently up to 60 minutes and defaults to 5 minutes", so clients must reconnect. Session affinity is "best effort". Instances share no state, so cross-instance sync needs Redis Pub/Sub (Memorystore) or Firestore real-time updates. **"A Cloud Run instance that has any open WebSocket connection is considered active"** and is billed as instance-based. Up to 1,000 concurrent connections per container. Don't enable end-to-end HTTP/2 for WebSocket services — [Cloud Run WebSockets docs](https://docs.cloud.google.com/run/docs/triggering/websockets)

**Cloudflare Workers + Durable Objects**
- Workers Free: 100,000 requests/day, 10 ms CPU per invocation. Workers Paid: **$5/mo minimum**, 10M requests/mo included then $0.30/M, 30M CPU-ms then $0.02/M CPU-ms, up to 5 min CPU per invocation. **A WebSocket connection is billed as one request (the Upgrade), and messages through a Worker don't count as requests.** Duration isn't billed. **No egress/bandwidth charges** — [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/)
- Durable Objects are available on Free (SQLite backend only): 100k requests/day, 13,000 GB-s/day, 5M rows read/day, 100k rows written/day, 5 GB stored. Paid: 1M requests/mo then $0.15/M; 400K GB-s then $12.50/M GB-s; rows written 50M then $1/M; storage 5 GB-mo then $0.20/GB-mo. **Incoming WebSocket messages are billed at 20:1 as requests; outgoing messages are free** — [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)
- **WebSocket Hibernation**: the DO is evicted from memory while clients stay connected to Cloudflare's edge, and "Billable Duration (GB-s) charges do not accrue during hibernation". Use `ctx.acceptWebSocket()`. Auto ping/pong doesn't wake the DO. Per-socket attachments are limited to 16,384 bytes. Hibernation works **only when the DO is the WebSocket server**, while an outbound WebSocket keeps the DO alive (up to 15 min). `setTimeout`/`setInterval` prevent hibernation. **Code deploys disconnect all WebSockets** — [DO WebSockets best practices](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)
- DO limits: 32 MiB max received WebSocket message; soft limit 1,000 requests/s per object; CPU 30 s default (up to 5 min) per request or message; SQLite storage 10 GB per object on Paid; unlimited number of objects — [DO limits](https://developers.cloudflare.com/durable-objects/platform/limits/)
- Workers TCP sockets are outbound (`connect()`). Inbound TCP is accepted only from another Worker via service binding: "Support for handling inbound TCP connections from other sources is coming soon." Port 25 is blocked, and so are localhost/private IPs — [Workers TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/)

**Fly.io**
- shared-cpu-1x (iad/ewr, 30 days): 256 MB **$2.19/mo**, 512 MB $3.69, 1 GB $6.70. Stopped machines are billed $0.15/GB-mo of rootfs. Dedicated IPv4 **$2/mo**; shared IPv4 and Anycast IPv6 are free. Egress $0.02/GB (NA/EU), $0.04 (APAC/SA), $0.12 (Africa/India). Inbound free. TLS certs: first 10 single-hostname free, then $0.10/mo; **wildcard $1/mo**. **No general free tier for new orgs** (trial is 2 h of machine runtime or 7 days) — [Fly.io pricing](https://docs.fly.io/about/pricing)

**Small VPS (Hetzner)**
- Hetzner's cloud page (Oct 2026) shows "Cost-Optimized" (shared) **from €5.99/mo, marked "Currently not available"**, "Regular Performance" (shared) from €6.49/mo, and dedicated vCPU from €43.49/mo — [Hetzner Cloud](https://www.hetzner.com/cloud/)
- Third-party reports describe a 2026 price increase (effective 1 Apr 2026, all customers), e.g., CX22 €3.29 → €4.49. The sources disagree on dates and models and say the cheapest tier became unavailable around June–Sept 2026 — [agentdeals Hetzner 2026](https://agentdeals.dev/hetzner-pricing-2026); [Cloudron forum](https://forum.cloudron.io/topic/15111/hetzner-price-increases-by-20-30-other-hosting-providers-soon-to-follow/29); [falcao.org](https://falcao.org/posts/hetzner-not-cheap-anymore/)
- An HN "self-hosted ngrok alternative" thread (surfaced in search results; I couldn't identify the exact thread, and the tnnl thread is the closest match I fetched) mentions a Docker-based tunnel on a Hetzner VPS where "the cheapest server costs 3€ per month" — [HN: tnnl](https://news.ycombinator.com/item?id=47088419) (low-confidence attribution; the figure likely predates the 2026 increase)

**Supabase Realtime**
- Limits: Free 200 concurrent connections, 100 msgs/s, 256 KB broadcast payload. Pro 500 concurrent, 500 msgs/s, 3,000 KB payload. Team 10,000 concurrent. 72 h broadcast replay retention — [Supabase Realtime limits](https://supabase.com/docs/guides/realtime/limits)
- Pricing: Pro from $25/mo (500 peak connections then $10 per 1,000; 5M messages then $2.50/M). Free includes 2M messages. Edge Functions: 500K (Free) / 2M (Pro) invocations. **"Free projects are paused after 1 week of inactivity."** — [Supabase pricing](https://supabase.com/pricing)

**Platform domains on the Public Suffix List** (matters for cookie isolation and per-subdomain reputation): `trycloudflare.com`, `workers.dev`, `pages.dev`, `fly.dev`, `web.app`, `firebaseapp.com`, `*.run.app`, `ngrok.app`, `ngrok-free.app`, `ngrok.dev`, `ngrok-free.dev`, `ngrok.io`, `ngrok.pizza`, and `ts.net` are listed in the PSL private section. `devtunnels.ms`, `loca.lt`, `zrok.io`, `serveo.net`, `webhook.site`, and `hkdk.events` were not found in my chunked reading — [Public Suffix List](https://raw.githubusercontent.com/publicsuffix/list/main/public_suffix_list.dat)

### Inferences
- **(a) Webhook capture on Firebase: feasible.** A 2nd-gen HTTPS function (called on its `run.app`/`cloudfunctions.net` URL, not via Hosting, to avoid the 60 s cap) writes each request to Firestore. The desktop app holds a Firestore snapshot listener and forwards to localhost. Approximate cost per webhook: 1 invocation + 1 write + 1 read per listening client. The free quotas (2M invocations/mo, 20K writes/day ≈ 600K/mo) cover hobby use at $0, but **Blaze with a card is mandatory**. Downsides: cold starts on the capture path (Shopify allows only 5 s total), Firestore's 1 MiB document limit (from general knowledge, not verified here) versus 32 MB request bodies (large payloads would need Cloud Storage), and the response must be generic or pre-configured. Live "respond from localhost" isn't practical.
- **(b) Reverse tunnel on Firebase: not realistic.** No documented WebSocket support in Functions, a 60-min cap, the Hosting 60 s cap, and RTDB's 100-connection Spark limit. Emulating a tunnel through Firestore (function waits for the desktop to write a response doc) adds hundreds of ms to seconds of latency and bills function time while waiting. It's fine for a "webhook relay", unacceptable for browsing a dev site or HMR.
- **Cloud Run**: works technically, but an always-connected desktop client keeps an instance "active". Billing continuous CPU for even one idle tunnel likely exceeds hobby budgets unless many tunnels share one instance (up to 1,000 connections), and the 60-min timeout forces hourly reconnects. Its best-effort affinity also means a request landing on instance B must reach the tunnel socket on instance A through Redis/Firestore pub/sub, adding complexity and latency.
- **Cloudflare DO cost model (indicative)**: one DO per tunnel/project acts as the WebSocket server for the desktop client, so hibernation applies and idle tunnels cost ~$0 duration. Per forwarded HTTP request: 1 Worker request + 1 DO request + the client's response message billed at 1/20 request. So 1M webhooks/month ≈ 1M Worker requests (within 10M) + ~1.05M DO requests (≈$0.01 over the 1M included). Indie scale is ≈ **$5/mo flat, with no egress fees**. The free plan's 100k DO requests/day (~3M/mo) could even serve a beta. Caveats: deploys drop all tunnel sockets (clients must auto-reconnect), the 10 ms CPU cap on the Free plan, and **no inbound raw TCP**, so HTTP/WS/SSE only.
- **Fly.io / VPS**: needed for raw TCP/UDP (ngrok-style `tcp://host:port`), QUIC transport, or TLS passthrough. Minimum is ≈ $2.19 (256 MB) + $2 dedicated IPv4 ≈ **$4.2/mo** plus $0.02/GB egress on Fly. Hetzner is ≈ €4.5–6.5/mo after 2026 increases, with stock caveats. Fly bills wildcard certs at $1/mo, which suits `*.tunnel.devall.app`-style routing.
- **Supabase Realtime** could relay captured webhooks (broadcast, 256 KB payload on Free). But **free projects pause after 1 week of inactivity**, which is fatal for an always-on relay unless on Pro ($25/mo), and it isn't designed for byte-stream tunnelling.
- **Recommended architecture** (inference): Cloudflare Worker (subdomain routing, abuse checks) → Durable Object per project (WebSocket hub with hibernation, short-lived SQLite buffer for webhooks received while the desktop is offline, replayed on reconnect) → desktop Rust client over WSS:443. Add an optional Fly.io/VPS "TCP relay" later for paid TCP/UDP. Keep Firebase out of the data path, or use it only for auth/account metadata if the author prefers the Firebase ecosystem.

### Gaps
- **Cloud Run unit prices and free tier were not retrieved** (pricing page rendered empty). The WebFetch model offered unverified training-data figures (≈2M requests, 180K vCPU-s, and 360K GiB-s free/month; ≈$0.000024/vCPU-s; ≈$0.0000025/GiB-s). Treat these as **unverified**.
- Firestore per-operation prices beyond the free tier ("Google Cloud pricing") weren't captured.
- No official statement was found on whether Cloud Functions 2nd gen (Functions Framework) supports WebSocket upgrades. The assessment rests on its absence from the quotas docs and on the Hosting timeout.
- Cold-start latency for 2nd gen functions wasn't measured or sourced.
- Hetzner's 2026 prices and included traffic are inconsistent across sources, and Hetzner's own page showed only "from" prices.
- Whether Cloudflare's free Universal SSL covers wildcard subdomains on a custom zone for Workers routes wasn't verified.

---

## 5. Abuse and security concerns for running a public tunnel/webhook service, and provider mitigations

### Takeaway
Any free public-URL service will be used for phishing and malware delivery. ngrok, zrok, Serveo, and Microsoft Dev Tunnels all put browser interstitials in front of free-tier HTML. ngrok adds content scanning and instant bans. Cloudflare's no-account TryCloudflare was used in large RAT-delivery campaigns. The standard mitigation stack: an account (and/or card) for public tunnels, an interstitial on HTML for unpaid users (skippable via header for API clients), content heuristics, abuse reporting and fast takedown, rate/quota limits, and putting the tunnel domain on the Public Suffix List.

### Cited Findings
- ngrok enabled the interstitial in 2022 "for all new, free users serving content through a browser" to eliminate phishing — [ngrok abuse page](https://ngrok.com/abuse) (via search summary) **[pre-2025, may be outdated]**
- ngrok (Feb 28, 2023): unauthenticated users can't serve HTML at all. Free authenticated accounts serving HTML trigger a warning that visitors "should not enter any sensitive information", showing the host's IP. Fuzzy matching on free-account HTML detects phishing, flagged accounts are removed immediately, abuse@ngrok.com reports lead to bans, and paid accounts are exempt. ngrok cites 6M+ developers — [ngrok: combating phishing](https://ngrok.com/blog/how-ngrok-actively-combats-phishing-attacks) **[pre-2025, may be outdated]**
- Current ngrok interstitial: HTML browser traffic on free only. Visit-cookie lasts 7 days. API/programmatic clients are unaffected. Bypass via the `ngrok-skip-browser-warning` header or a custom User-Agent — [ngrok free plan limits](https://ngrok.com/docs/pricing-limits/free-plan-limits)
- zrok added an interstitial for free-tier public shares (rollout 8 Aug, post ~late July 2024) citing "abuse, phishing, and other similar types of issues". It is shown only to interactive browsers (curl and HTTP libraries skip it), resets weekly, isn't shown for paid or private shares, and is configurable per frontend for self-hosters — [zrok is growing up](https://blog.openziti.io/zrok-is-growing-up) **[pre-2025, may be outdated]**. zrok's current pricing says unverified free accounts see the interstitial and "add a credit card anytime to remove interstitial pages" — [zrok pricing](https://zrok.io/pricing/)
- Serveo's free plan includes an "interstitial warning", which Pro removes. The site has a Report Abuse form — [serveo.net](https://serveo.net/)
- Microsoft Dev Tunnels are private by default (account auth to access) and show warnings on visited URLs — [Dev Tunnels overview](https://learn.microsoft.com/en-us/azure/developer/dev-tunnels/overview); [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling)
- Tailscale Funnel requires an explicit `funnel` node attribute in the tailnet policy file, gating who can expose services — [Tailscale Funnel](https://tailscale.com/kb/1223/funnel)
- Proofpoint (Aug 2024) tracked financially motivated campaigns abusing **TryCloudflare** (no-account tunnels) to deliver Xworm, AsyncRAT, VenomRAT, GuLoader, and Remcos. Volumes ranged from hundreds to tens of thousands of messages hitting thousands of orgs, and tunnels undermined static blocklists — [Proofpoint](https://www.proofpoint.com/us/blog/threat-insight/threat-actor-abuses-cloudflare-tunnels-deliver-rats) **[pre-2025, may be outdated]**. Cloudflare responded by disabling malicious tunnels, per Anvilogic's summary of BleepingComputer — [Anvilogic](https://www.anvilogic.com/threat-reports/cloudflare-tunnels-rat-abuse); [BleepingComputer](https://www.bleepingcomputer.com/news/security/hackers-abuse-free-trycloudflare-to-deliver-remote-access-malware/)
- Collateral blocking: an HN commenter's vendor "blocked ngrok URLs for webhook testing as unreliable" — [HN: rustunnel thread](https://news.ycombinator.com/item?id=47425918)
- webhook.site free URLs are readable by anyone with the ID and cap at 100 requests. Paid URLs with extraordinary traffic may be auto-blocked — [webhook.site docs](https://docs.webhook.site/). smee.io channels are unauthenticated (anyone with the ID can read payloads) — [probot/smee.io](https://github.com/probot/smee.io)
- Major tunnel and PaaS domains (ngrok's six domains, `ts.net`, `trycloudflare.com`, `workers.dev`, `fly.dev`, `web.app`, `firebaseapp.com`, `*.run.app`) are registered in the PSL private section. This scopes cookies per subdomain and lets browsers and reputation systems treat each subdomain as its own "site" — [Public Suffix List](https://raw.githubusercontent.com/publicsuffix/list/main/public_suffix_list.dat)

### Inferences
- **For /dev/all's relay**:
  - Require sign-in (e.g., GitHub OAuth) before issuing public tunnel URLs. Keep anonymous use limited to webhook *capture* URLs that never serve HTML: respond with a fixed or user-configured body and force `Content-Type` to non-HTML, or add `Content-Security-Policy: sandbox`.
  - Apply an ngrok-style interstitial to `text/html` responses for free tiers, skippable by header or non-browser UA, so webhooks and APIs are unaffected.
  - Use a **dedicated registrable domain** for tunnels (separate from the marketing/app domain) and submit it to the PSL. That way one abusive subdomain doesn't get the whole domain blocklisted, and tunnel subdomains can't set cookies on each other.
  - Add rate and bandwidth quotas per account, an abuse@ address with a fast kill switch per subdomain, and logging of client IPs and account IDs for takedown.
  - Optionally keep a card-verified tier for raw TCP (ngrok requires card verification for free TCP addresses).
- Cloudflare-hosted relays inherit some Cloudflare edge protections, but you remain responsible for abuse on your own zone. Cloudflare's TryCloudflare experience shows that no-account tunnels attract malware operators.

### Gaps
- No primary-source data was found on Google Safe Browsing or corporate firewalls blocklisting ngrok/loca.lt domains (only an anecdotal HN mention).
- The reason for localtunnel's "tunnel password" reminder page on loca.lt (believed to be anti-phishing) could not be confirmed from the pages fetched.
- Legal and regulatory obligations for operating a relay (DMCA/abuse handling, data retention of webhook payloads with PII, GDPR) were not researched.

---

## 6. Developer complaints about ngrok and webhook testers, and features that would delight

### Takeaway
The recurring complaints are free-tier quotas and interstitials (ngrok), URLs that change or expire (Quick Tunnels, Pinggy 60 min, localhost.run rotation, webhook.site 7 days and 100 requests), unreliability (localtunnel), vendor blocking of shared tunnel domains, setup friction ("50 lines of TOML"), and distrust of VC-driven pricing changes. What developers ask for: stable or predictable subdomains (per developer and per project), auto-reconnect, port 443/HTTP/2 transport that survives corporate networks, TCP/TLS plus custom domains, and built-in inspection with replay.

### Cited Findings
- A tnnl author built it because ngrok's free tier "got too restrictive", and alternatives "either don't do HTTP subdomain routing or need 50 lines of TOML". tnnl offers `--inspect` and `tnnl replay 3` (posted ~8 months ago) — [HN: tnnl](https://news.ycombinator.com/item?id=47088419)
- From the rustunnel HN thread (~7 months ago):
  - Complaints: a vendor blocked ngrok URLs; ngrok "shifted toward AI and enterprise routing"; "VC-driven enshittification" (ngrok's $50M funding); signup requirement; localtunnel reliability issues
  - Wishes: fixed or predictable subdomains, low latency, an auto-reconnecting daemon, port 443 and HTTP/2 streaming ("non-443 ports are a pain in corporate networks"), and Colab support
  - [HN: rustunnel](https://news.ycombinator.com/item?id=47425918)
- 2023 Ask HN (Pinggy founder): commenters called TCP/TLS tunnels and custom domains "absolute must haves" for OAuth/webhook testing. They also wanted fixed subdomains per team member auto-assigned so third-party configs don't need updating — [Ask HN](https://news.ycombinator.com/item?id=37166196) **[pre-2025, may be outdated]**
- A 2026 comparison:
  - ngrok's free interstitial is "awkward for client demos", and "quotas, not uptime, are the bottleneck"
  - Quick Tunnels are "not durable" and unsuitable "for webhooks you forget about"
  - Pinggy free sessions expire after 60 minutes, and its random subdomains change on reconnect
  - localhost.run hostnames rotate after a few hours
  - Funnel only supports `ts.net` (custom domains are a community request)
  - Dev Tunnels have higher latency and occasional drops
  - [merginit 2026](https://merginit.com/blog/19062026-free-developer-tunnels-comparison)
- Aggregators disagree on ngrok's own limits and prices (e.g., a 2-hour session cap and "Personal $8/Pro $20/Enterprise $39" tiers that don't match ngrok's current page). This confusion is itself a pain point — [devtoolpicks 2026](https://devtoolpicks.com/blog/best-ngrok-alternatives-indie-hackers-2026); [LocalXpose blog](https://localxpose.io/blog/best-ngrok-alternatives); [ngrok pricing](https://ngrok.com/pricing)
- A vendor (self-interested) argues that tunnels only work while your computer is on, and a permanent URL needs hosting — [livemy.app](https://livemy.app/blog/ngrok-alternatives)
- Webhook-tester limits developers hit:
  - webhook.site free: 100 requests, 7-day expiry, public — [webhook.site docs](https://docs.webhook.site/)
  - Beeceptor free: 50 req/day — [Beeceptor pricing](https://beeceptor.com/pricing/)
  - Webhook Relay free: 150/month and no tunnels — [Webhook Relay pricing](https://webhookrelay.com/pricing/)
  - ngrok Full Capture truncates at 10 KB and keeps 24 h on free — [ngrok Traffic Inspection](https://ngrok.com/docs/obs/traffic-inspection)
  - smee stores nothing, so events sent while offline are lost — [probot/smee.io](https://github.com/probot/smee.io)
- Stripe documents that mixing up CLI and Dashboard `whsec_` secrets and frameworks mutating the raw body are the most common causes of signature failures — [Stripe: signature errors](https://docs.stripe.com/webhooks/signature)

### Inferences
Features that would delight in /dev/all (synthesized from the complaints above):
1. **Stable, per-project URL by default** (e.g., `<project>-<user>.devall.dev`) that survives restarts. Webhook configs in Stripe/GitHub never need updating. Team members get their own predictable subdomain.
2. **Offline buffering**: the relay stores webhooks while the laptop sleeps and delivers or replays them on reconnect, fixing the "tunnel only works while your computer is on" problem for webhooks.
3. **Local-first unlimited history**: full bodies, no 10 KB truncation, searchable, diffable, exportable as HAR/cURL/Postman, kept forever on disk, unlike 24 h–7 day cloud retention.
4. **Replay with edit plus re-sign**: provider-aware signature verification (Stripe, GitHub, Shopify, Twilio, Slack, Standard Webhooks) with a "why did verification fail" explainer (raw-body mutation, wrong secret, clock skew, URL mismatch for Twilio), and re-signing on replay to beat 5-minute tolerances.
5. **Custom/mock responses and "respond from localhost" toggle**: capture-only mode (fast 200 ack for Shopify's 5 s budget) vs live pass-through.
6. **No interstitial for webhooks/APIs, HTTPS on 443 with WebSocket/SSE/HTTP-2 streaming**, and auto-reconnect with backoff, covering Quick Tunnels' SSE gap and corporate-network complaints.
7. **Provider sample payloads / trigger library** (as Hookdeck's Webhooks Library does), tied to the project.
8. **Transparent, flat pricing** with a generous free tier possible on a Cloudflare DO architecture (~$5/mo infra baseline). Optionally, open-source the relay (AGPL) and allow self-hosting to address "rug pull" distrust.
9. **Pluggable providers**: a native relay by default, plus "use my ngrok token" (ngrok-rust) or "use cloudflared" as fallbacks.

### Gaps
- No large-sample survey data (e.g., Stack Overflow survey) on tunnel or webhook tool satisfaction was found. Complaint evidence is anecdotal (HN threads with few comments, blogs).
- Reddit threads weren't retrieved: the web search budget was exhausted mid-research, so further Reddit/HN sweeps weren't possible in this session.

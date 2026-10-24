# /dev/all relay (Group 2 — hosted, or self-hosted for free)

The relay gives a laptop a public HTTPS address — the ngrok replacement — and a webhook inbox
that keeps receiving while the laptop is asleep, plus the hosted extras: custom domains for
tunnels, raw TCP tunnels, publicly hosted mocks, shareable request/response snapshots,
scheduled monitors and cloud load-generator regions. It is the only part of /dev/all that runs on servers, so it is the only part
with a price: a generous **free tier**, a **Supporter tier (~$4/month)** priced near cost, and
**self-hosting with every feature free**.

- `worker/` — Cloudflare Worker + Durable Objects: the tunnel edge and the webhook inbox (TypeScript).
- `firebase/` — accounts and billing only: Firebase Auth, Firestore entitlements, Stripe webhook,
  subdomain claims, and the hosted sign-in page. Firebase never carries tunnel traffic.
- `PROTOCOL.md` — the versioned wire protocol between the edge and the desktop client.
- `e2e/` — live end-to-end checks: the real Worker under `wrangler dev --local` driven by the real
  CLI (`node relay/e2e/run.mjs`, see `e2e/README.md`).
- Desktop client: `crates/devall-relay` (Rust), UI in `apps/desktop/src/features/tunnels`.

License: **AGPL-3.0** (`LICENSE`) — anyone may run, study and change the relay, and a modified
relay offered to the public must publish its source (section 13), which prevents closed rebrands.
The desktop app and CLI keep their own license.

## Architecture

```
 visitor / webhook sender                                   developer's laptop
 ────────────────────────                                   ──────────────────
 https://brave-otter-4821.devall.dev/x ─┐                   /dev/all (devall-relay)
 https://hooks.devall.dev/<id>/stripe ──┤                     │  wss://relay.devall.dev/connect
                                        ▼                     │  (one WebSocket on 443,
                        ┌──────── Cloudflare Worker ───────┐  │   multiplexed streams)
                        │ host routing · auth · quotas     │◄─┘
                        │ interstitial · abuse controls    │
                        └──┬───────────────┬───────────────┘
               idFromName(sub)      idFromName(inbox)      idFromName(uid)
                        ▼               ▼                       ▼
                 ┌─ TunnelDO ─┐   ┌─ InboxDO ──────┐     ┌─ AccountDO ─────┐
                 │ client WS  │   │ SQLite buffer  │     │ bandwidth ledger │
                 │ (hibernate)│   │ ordered, acked │     │ live tunnels     │
                 │ HTTP/WS/TCP│   │ delivery       │     │ inbox ids, mock/ │
                 │ streams    │   │                │     │ share/monitor ids│
                 └────────────┘   └────────────────┘     └─────────────────┘
   ┌─ MockDO (per name) ─┐  ┌─ ShareDO (per link) ─┐  ┌─ MonitorDO (per monitor) ─┐
   │ stubs, __files,     │  │ snapshot, expiry     │  │ requests, encrypted       │
   │ scenarios, journal  │  │ alarm → deleted      │  │ secrets, results, alerts  │
   └─────────────────────┘  └──────────────────────┘  └───────────────────────────┘
        <name>.mock.<d>          share.<d>/<id>          cron */5 → due alarms
   ┌─ LoadDO (one per region of a cloud load run, newUniqueId + locationHint) ───────┐
   │ open-model arrival loop · fetch() to verified targets · log-linear histograms   │
   └─────────────────────────────────────────────────────────────────────────────────┘
        POST /v1/load/runs → segments of ≤ 10 s fanned out to every region, merged
                        ▲ KV: reserved subdomains, plan overrides, blocked users,
                        │     custom hosts (host:<h>), monitor index (mon:<id>)
 Firebase (accounts/billing only): Auth ── Firestore entitlements ── Functions
   stripeWebhook (Stripe → plan claim + entitlements/{uid} + KV plan:<uid>)
   claimSubdomain / releaseSubdomain (Supporter → Firestore + KV sub:<name>)
   addCustomDomain / removeCustomDomain / refreshCustomDomain (Cloudflare for SaaS → KV host:<h>)
   Hosting: signin.html (desktop loopback sign-in), upgrade.html (Stripe Payment Link)
```

- **Tunnels.** The desktop opens `wss://relay.<domain>/connect` with a Firebase ID token (hosted)
  or a static token (self-hosted). The Worker verifies it (RS256 against Google's securetoken
  JWKS — no Firebase SDK at the edge), picks the subdomain and hands the socket to the
  subdomain's `TunnelDO` via the WebSocket Hibernation API: an idle tunnel costs nothing, and the
  heartbeat is answered by the runtime's auto-response without waking the object. Public requests
  to `https://<sub>.<domain>` reach the same object, which frames them onto the client socket as
  streams with per-stream credit (`PROTOCOL.md`). Bodies stream both ways byte-for-byte;
  WebSockets are passed through; the exact public URL travels with every request.
- **Webhook inbox.** `https://hooks.<domain>/<inbox-id>/…` lands in an `InboxDO`, which stores the
  raw request in its SQLite storage, answers right away with the configured response (default
  `200 {"ok":true}`) and pushes events to the desktop in order. Events are deleted only when the
  desktop acknowledges them after its handler stored them (at-least-once; the client dedupes).
  A tunnel can also route chosen paths (e.g. `/webhooks/`) into the inbox while it is offline.
- **Accounts.** `AccountDO` (one per user) keeps the monthly bandwidth ledger, the live-tunnel
  registry (concurrency limits) and the inbox id per project. Plans come from the Firebase custom
  claim `plan`, overridden by `plan:<uid>` in KV so upgrades/cancellations apply immediately.
- **Custom domains.** A customer points `api.example.com` (CNAME) at `custom.<domain>`, the
  Cloudflare for SaaS fallback origin. `addCustomDomain` creates the custom hostname (Cloudflare
  validates control through the CNAME and issues/renews the certificate), stores it in Firestore
  and writes `host:<hostname> → {sub, uid}` to KV. The Worker routes any host outside its domain
  through that map (static `CUSTOM_HOSTS` on self-hosted relays) to the tunnel — only if the tunnel
  is connected by the same account.
- **Raw TCP tunnels.** Workers can't accept inbound TCP, so it's TCP over WebSockets (like
  `cloudflared access tcp`): the publisher's TunnelDO bridges each consumer socket (`/tcp`) to a
  new stream on the publisher's socket, with end-to-end credit (PROTOCOL.md §2.6). Consumers
  authenticate with the owner's account or the tunnel's `dtcp_…` access secret.
- **Hosted mocks.** `PUT /v1/mocks/<name>` uploads the project's WireMock mappings + small
  `__files` into a `MockDO` (SQLite); it matches and templates a documented WireMock subset
  (below) at `https://<name>.mock.<domain>` / `https://mock.<domain>/<name>/`, keeps scenario
  state and the last N requests (the journal the desktop reads).
- **Shared snapshots.** The desktop redacts a history entry (secret values, auth/cookie/API-key
  headers, token-like query parameters) and uploads it; `ShareDO` renders a read-only page at
  `https://share.<domain>/<id>` (cURL, `.req.yaml` and JSON downloads) and deletes itself at
  expiry.
- **Scheduled monitors.** A Cron Trigger (`*/5 * * * *`) lists `mon:<id>` keys (interval in the
  key's metadata) and arms each due `MonitorDO`'s alarm; the alarm runs the saved requests and
  assertions, stores the result (last N, uptime) and posts to a webhook / Slack on state changes.
  Retries of a cron slot are skipped by slot (the trigger's scheduled time), not by wall clock.
- **Cloud load-generator regions.** `POST /v1/load/runs` takes a test's open-model profile (built
  by `devall load cloud run` / the Load page from saved requests, resolved on the user's machine),
  checks that every target host is verified, and fans out to one `LoadDO` per region, created with
  `newUniqueId()` and a `locationHint` (`wnam`, `enam`, `sam`, `weur`, `eeur`, `apac`, `oc`, `afr`,
  `me`). Each region runs its share of the arrival schedule with `fetch()` (latency from the
  intended start, so it is coordinated-omission free) into a mergeable log-linear histogram (≤ 1.6 %
  error). The run is split into segments of ≤ 10 s and ≤ 800 requests per region — one RPC each, far
  below per-invocation subrequest limits — and the kill switch and the stop button act between
  segments. The coordinator merges the regions into the fields of a devall-load run summary; the
  client evaluates thresholds and stores it like a local run (regions appear as `cloud:<region>
  (<colo>)` agents). **Placement is best-effort:** `locationHint` only suggests where an object is
  first created and objects never move; the region's data center is recorded from a
  `/cdn-cgi/trace` probe (`LOAD_COLO_PROBE`) when available, `null` otherwise. A Worker invocation
  keeps at most 6 outgoing connections open, so each region's concurrency is small; requests beyond
  that queue, which shows up honestly as latency (and a warning when the planned rate wasn't met).

## Tiers

| | Free | Supporter (~$4/mo) | Self-hosted |
|---|---|---|---|
| Subdomain | random `adjective-noun-NNNN`, kept across reconnects for the session | up to 5 reserved names (or random) | any name |
| Session | 8 h, then a new random URL | 24 h, reconnects transparently | unlimited |
| Bandwidth | 1 GB / month | 50 GB / month | unlimited |
| Tunnels at once | 1 | 5 | unlimited |
| Request body | 10 MB | 100 MB | unlimited |
| Browser warning page | yes, once per 7 days, HTML navigations only | no | off by default |
| Webhook inbox | 1 per account, 24 h buffer, 500 events/day | one per project (20), 30 days, 10 000/day | unlimited, 90 days |
| Rate limit per visitor IP | 300 req/min | 1 200 req/min | none |
| Custom domains | — | 2 (CNAME, managed certificates) | any (`CUSTOM_HOSTS`) |
| Raw TCP tunnels (publish) | — | yes, 20 connections per tunnel | yes, unlimited |
| Raw TCP tunnels (connect) | yes (with the publisher's secret) | yes | yes |
| Hosted mocks | 1 · 50 stubs · 1 MB · 5 000 req/day · journal 50 | 10 · 1 000 stubs · 20 MB · 100 000 req/day · journal 500 | unlimited (100 MB each) |
| Shared links | 20 active · 256 KB · ≤ 7 days | 500 · 5 MB · ≤ 90 days | unlimited · 20 MB · ≤ 1 year |
| Scheduled monitors | — | 20 · every 5 min · 20 requests · 2 016 results | unlimited · every 5 min (cron) · 50 requests |
| Cloud load-generator regions (verified hosts only) | — | ≤ 3 regions · ≤ 60 s · ≤ 100 req/s in total · 20 runs/day · public hosts | all 9 regions · ≤ 600 s · no rate cap · unlimited runs · private / loopback hosts too |

Limits live in `worker/src/plans.ts` and can be overridden without code changes with the
`LIMITS_JSON` variable. Webhooks, API clients and `curl` never see the warning page; it shows
only for top-level browser navigations (`Accept: text/html`, a `Mozilla/` user agent,
`Sec-Fetch-Dest: document`) without the `devall_continue` cookie or the
`devall-skip-browser-warning` header.

## What it costs to run

Cloudflare Workers Paid is **$5/month** and includes 10 M Worker requests, 1 M Durable Object
requests, 400 000 GB-s of DO duration, 5 GB of DO storage and **no bandwidth charges**. Hibernated
sockets accrue no duration, and outgoing WebSocket messages are free (incoming ones bill 20:1).

A proxied request costs about 1 Worker request + 1 DO request + ~0.15 DO request-equivalents of
client frames, a few ms of CPU, and ~0.1 s × 128 MB of DO duration.

| Monthly load | Workers | DO requests | DO duration | Total |
|---|---|---|---|---|
| Hobby: 50 users × 10 k requests (0.5 M) | included | included | included | **$5** |
| 1 000 active free users × 20 k requests (20 M) | 10 M over × $0.30/M = $3 | 23 M over × $0.15/M = $3.5 | ~250 k GB-s, included | **≈ $12** |
| + inbox storage (500 events/day × 5 KB × 1 000 users) | | | ~2.5 GB, included | — |

The hosted extras stay inside the same $5 base at small scale:

- **TCP tunnels**: a consumer socket costs like a public WebSocket — hibernated while idle, one
  incoming-message DO request per 20 frames while busy, no bandwidth charge.
- **Hosted mocks**: one Worker + one DO request per call plus a journal row; a free mock capped at
  5 000 requests/day is ≤ 150 000 requests/month (≈ $0.07 over the included quota in the worst
  case) and ≤ 1 MB of storage. Delays are capped at 10 s because a sleeping request bills DO
  duration.
- **Shared links**: one DO with ≤ 256 KB (free) of storage until it expires; viewing is one request.
- **Monitors** (Supporter): the cron fires 8 640 times a month (one KV list each, $0.005/1 000); a
  Supporter's 20 monitors every 5 minutes with 20 requests each are ≈ 175 000 DO requests (alarms)
  + 3.5 M subrequests (subrequests are free; CPU ~1 ms each) — ≈ $0.03/month in DO requests over the
  quota.
- **Cloud load regions** (Supporter): a maximal run (3 regions, 60 s, 100 req/s) is 6 000
  subrequests (free), ≈ 25 DO requests (segments) and ≈ 3 × 60 s × 128 MB ≈ 23 GB-s of DO duration
  plus ~1 ms CPU per request; 20 such runs a day are ≈ 14 000 GB-s a month — inside the 400 000
  GB-s included, so even heavy Supporters stay well within the $4.
- **Custom domains**: Cloudflare for SaaS includes 100 custom hostnames on paid plans, then
  $0.10/hostname/month — at 2 per Supporter that's ≤ $0.20 per Supporter, inside the $4.
- **`*.mock.<domain>` certificate** (only for host-style mock URLs): Universal SSL covers one
  level, so `<name>.mock.<domain>` needs Advanced Certificate Manager (**$10/month**). The default
  `MOCK_URL_STYLE = "path"` hands out `https://mock.<domain>/<name>/` instead (free); set
  `MOCK_DOMAIN` to a second zone (e.g. `devall-mock.dev`, its own free Universal SSL) for
  host-style URLs without ACM.

Firebase stays on the free quotas (Auth, Firestore reads of one doc per sign-in, a handful of
function invocations per subscription event or custom-domain change, a 30-minute scheduled refresh
of pending custom domains; Hosting serves three static pages). Stripe takes
≈ 2.9 % + $0.30 per charge, so a $4 Supporter nets ≈ $3.58: **two Supporters cover the base
cost, ~4 cover the 1 000-user scenario above.** Annual billing ($36/yr) cuts the fixed fee share.

## Deploy the hosted relay (operator)

Prerequisites: a domain dedicated to tunnels (not your marketing/app domain), a Cloudflare account
on Workers Paid, a Firebase project on Blaze (Functions need it; usage stays in the free quota),
a Stripe account, Node 22.

### 1. Cloudflare (edge)

```sh
cd relay/worker
npm install
npx wrangler login
npx wrangler kv namespace create RELAY_KV          # paste the id into wrangler.toml
# DNS on the tunnel zone (proxied/orange cloud): AAAA records to 100:: for `*`, `relay`, `hooks`
# Edit wrangler.toml: routes (zone), PUBLIC_DOMAIN, FIREBASE_PROJECT_ID, ABUSE_EMAIL, UPGRADE_URL, SOURCE_URL
npx wrangler secret put ADMIN_TOKEN                # long random string, for takedowns
npx wrangler secret put MONITOR_SECRET_KEY         # 32+ random bytes (base64); encrypts monitor secrets
npx tsc --noEmit && npx vitest run                 # all local (Miniflare)
npx wrangler deploy                                # applies DO migrations v1–v3 (MockDO, ShareDO, MonitorDO, LoadDO) and the */5 cron
```

Universal SSL covers `*.example.dev` one level deep, which is exactly what tunnels use — and
`share.<domain>` and `mock.<domain>` (path-style hosted mocks) too. Optional: add a
`[[ratelimits]]` binding named `CONNECT_LIMITER` to rate-limit connection attempts per IP (also
applies to TCP consumer connections).

**Custom domains (Cloudflare for SaaS)** — on the tunnel zone:

1. SSL/TLS → Custom Hostnames → enable Cloudflare for SaaS (pay-as-you-go: 100 hostnames included).
2. DNS: proxied `custom` AAAA `100::` (the fallback origin) and set it as the **fallback origin**.
3. Add a Worker route `*/*` → `devall-relay` on the zone (Workers as the SaaS fallback origin), in
   addition to `*.devall.dev/*`.
4. Give the Firebase `CF_API_TOKEN` the extra permission **Zone → SSL and Certificates → Edit**
   (custom hostnames) next to Workers KV Storage → Edit, and set the function params below.

**Hosted mocks with host-style URLs** (optional): DNS `*.mock` AAAA `100::` (proxied) + an
Advanced Certificate for `*.mock.<domain>`, then `MOCK_URL_STYLE = "host"` — or a separate zone in
`MOCK_DOMAIN` with its own wildcard record and route.

### 2. Firebase (accounts + billing)

```sh
cd relay/firebase
npm install --prefix functions
npx firebase-tools@15.34.0 login
npx firebase-tools@15.34.0 use --add               # select the project; replaces .firebaserc's TODO
# Console: Authentication → enable Google, GitHub (needs a GitHub OAuth app) and Email link;
#          add the Hosting domain to Authorized domains.
# Edit hosting/config.js (firebaseConfig, Stripe Payment Link, customer portal link).
npx firebase-tools@15.34.0 functions:secrets:set STRIPE_WEBHOOK_SECRET
npx firebase-tools@15.34.0 functions:secrets:set CF_API_TOKEN   # Cloudflare token: Workers KV Storage → Edit (+ Zone SSL and Certificates → Edit for custom domains)
# functions/.env (plain config, no secrets): CF_ACCOUNT_ID=…  CF_KV_NAMESPACE_ID=…
#   CF_ZONE_ID=…  PUBLIC_DOMAIN=devall.dev  CUSTOM_DOMAIN_TARGET=custom.devall.dev   (custom domains)
npm --prefix functions run typecheck && npm --prefix functions test
npx firebase-tools@15.34.0 deploy --only functions,firestore:rules,hosting
```

### 3. Stripe

1. Product "Supporter", recurring price ≈ $4/month (and optionally $36/year).
2. A **Payment Link** for it → put its URL in `hosting/config.js` (`stripePaymentLink`). The
   upgrade page appends `client_reference_id=<firebase uid>`.
3. Customer portal (cancel / change card) → login link into `stripeCustomerPortal`.
4. Webhook endpoint → the `stripeWebhook` function URL (the `cloudfunctions.net`/`run.app` URL, not
   a Hosting rewrite) with events `checkout.session.completed`,
   `checkout.session.async_payment_succeeded`, `customer.subscription.created`,
   `customer.subscription.updated`, `customer.subscription.deleted`,
   `customer.subscription.paused`, `customer.subscription.resumed`; its signing secret is
   `STRIPE_WEBHOOK_SECRET`.

### 4. Desktop app

Fill in `crates/devall-relay/src/hosted.rs` (endpoint, Firebase Web API key, project id, sign-in
and upgrade page URLs, `FUNCTIONS_URL` for the custom-domain callables). Until then the app offers
self-hosting only.

### 5. Public Suffix List

Submit the tunnel domain to the PSL private section
(<https://github.com/publicsuffix/list>, "Submitting amendments"): add a `_psl` TXT record with the
pull-request URL, list `devall.dev` (your domain) with an explanation that subdomains belong to
different users. This isolates cookies between tunnels and keeps one abusive subdomain from getting
the whole domain blocklisted (ngrok, `workers.dev`, `fly.dev`, `ts.net` do the same). Expect weeks
of review; ship without it, but keep the domain dedicated to tunnels.

## Self-host (Cloudflare only, no Firebase, every feature free)

```sh
cd relay/worker
npm install
npx wrangler login
npx wrangler kv namespace create RELAY_KV          # paste the id into wrangler.toml
# wrangler.toml: routes/zone + PUBLIC_DOMAIN = your domain, AUTH_MODE = "static",
#                SOURCE_URL = where your copy of the code lives
# DNS (proxied): `*`, `relay`, `hooks` → AAAA 100::
npx wrangler secret put RELAY_TOKENS               # e.g. "alice:<32 random chars>,ci:<32 random chars>"
npx wrangler secret put ADMIN_TOKEN
npx wrangler deploy
```

In /dev/all: Tunnels → Self-hosting → "My relay", address `wss://relay.<your domain>`, paste a
token. Static-token users get the `selfhost` plan: any subdomain, no warning page, no quotas, and
every hosted extra: raw TCP tunnels, hosted mocks, shared links, monitors (set
`MONITOR_SECRET_KEY` to let monitors store secrets; alert webhooks may be plain `http://` on your
own network) and cloud load regions (every region, private/loopback targets allowed — still only
after verification; `CLOUD_LOAD = "off"` disables them). DNS: also `share` and `mock` (proxied
`100::`). Custom domains on your own relay: add `CUSTOM_HOSTS = '{"api.example.com":"my-box"}'` to
`wrangler.toml` and a Worker route `api.example.com/*` (or a Workers Custom Domain) for each
hostname in the same Cloudflare account — no Cloudflare for SaaS or Firebase needed. The CLI uses
the same relay: `devall tunnel http --port 3000 --relay wss://relay.example.com --token …`,
`devall tunnel tcp --port 5432 …`, `devall inbox listen`, `devall load cloud run … --regions weur`
(or `DEVALL_RELAY_URL` / `DEVALL_RELAY_TOKEN` instead of the flags).
Running a small relay for friends? Give them their own tokens and set
`STATIC_PLANS='{"friend":"free"}'` to apply free-tier limits per token; `INTERSTITIAL = "off"`
disables the warning page globally. The Workers free plan also works for light use (100 k
requests/day; SQLite-backed Durable Objects are available on it).

## Abuse policy

Free public URLs attract phishing and malware; the controls are deliberately boring:

- An account is required for any public URL (no anonymous tunnels); accounts can be blocked
  (`POST /admin/block-user?uid=…`), and a subdomain can be disabled instantly
  (`POST /admin/block?name=…`, both with `Authorization: Bearer $ADMIN_TOKEN`) — its socket gets
  GOAWAY `blocked` and visitors get 451.
- Free tunnels show browser visitors an interstitial warning with the host name and a **Report
  abuse** link (`https://relay.<domain>/report`, which mails `ABUSE_EMAIL`).
- Inboxes never serve active content: HTML/JS/SVG responses are downgraded to `text/plain` with
  `Content-Security-Policy: sandbox`.
- Per-visitor-IP rate limits, per-account bandwidth and concurrency limits, request-size limits.
- `X-Robots-Tag: noindex` on every tunnel and inbox response; random free names aren't recycled
  to another account for an hour.
- Reports are answered within a day; we log account ids and subdomains (not payloads) to act on them.

Hosted extras:

- **Custom domains**: Cloudflare only issues a certificate once the hostname's DNS points at us
  (control is proven by the CNAME); a domain only reaches the tunnel of the account that added it;
  blocking the subdomain (`/admin/block`) also takes its custom domains offline.
- **Raw TCP tunnels**: never anonymous — consumers need the owner's account or the tunnel's
  access secret (only its SHA-256 reaches the relay); per-IP connection rate limit, a cap on
  concurrent connections, bandwidth counted like HTTP; Supporter-only on the hosted relay.
- **Hosted mocks**: API traffic only — HTML / SVG / JavaScript content types are served as
  `text/plain`, every response carries `Content-Security-Policy: sandbox`, `nosniff` and
  `X-Robots-Tag: noindex`; per-IP rate limit, daily request cap, 1 MB request bodies, 10 s
  delays at most, size limits per plan, no proxying (`proxyBaseUrl` answers 501), names from the
  reserved list refused. The interstitial doesn't apply (API clients).
- **Shared links**: need an account (or a self-host token), expire (7 days free), can be revoked,
  are `noindex`, served with a strict CSP (a nonce'd copy button is the only script) and
  `Referrer-Policy: no-referrer`; ids are 22 random base62 characters (~131 bits).
- **Monitors**: Supporter only on the hosted relay; outbound requests are bounded by plan
  (interval, requests per run, 30 s timeout, 1 MB read), results never store response bodies or
  secret values, alerts only go to `https://` webhooks or `https://hooks.slack.com/…` (self-hosted
  relays may also use `http://`), manual runs at most every 30 s.
- **Cloud load-generator regions** (Supporter; free on a self-hosted relay) — a burst from shared
  edge IPs at someone else's URL would be a free DDoS service, so:
  - **Ownership verification is mandatory** (like loader.io / k6 cloud). `POST /v1/load/targets`
    hands out a per-account token; the relay fetches `<origin>/.well-known/devall-load-verification.txt`
    (HTTP 200, no redirects followed, ≤ 4 KB, must contain the token) or, failing that, looks up a
    DNS TXT record `_devall-load.<host>` over DNS-over-HTTPS (1.1.1.1; `LOAD_VERIFY_DNS = "off"`
    disables it). A file verification covers that `host:port`, a DNS one every port of the host.
    Verifications expire after **30 days**. Every request URL of a run must be covered, or the run
    is refused before anything is sent.
  - **Never private networks on the hosted relay**: IP literals in loopback, RFC 1918, CGNAT,
    link-local, multicast and reserved ranges (IPv4 and IPv6), `localhost`, `*.local`,
    `*.internal`, `*.lan`, `*.home.arpa` and single-label names are refused, as is the relay's own
    domain (no load loops through the relay). Self-hosted relays (`loadPrivateTargets`) may target
    private hosts — still only after verification.
  - **Bounded runs**: Supporter ≤ 3 regions, ≤ 60 s, ≤ 100 requests/s summed over regions (flow
    steps count), ≤ 20 runs per UTC day, one run at a time per account, request timeouts ≤ 30 s,
    bodies ≤ 64 KiB. All limits are in `plans.ts` (`LIMITS_JSON` overrides them; self-host defaults:
    every region, ≤ 600 s, no rate cap).
  - **Kill switch**: `CLOUD_LOAD = "off"`, or `POST /admin/load/disable` (`/admin/load/enable`) with
    the admin token — new runs get `503 load_disabled` and running ones stop at their next segment
    (≤ 10 s). Blocking an account (`/admin/block-user`) also stops it from starting runs.
  - **Identifiable traffic**: every request carries `x-devall-load: <run id>` and a
    `devall-load-cloud/1` user agent pointing at the verification file, so a target's operator can
    recognise (and block) it; run ids, account ids and targets are what we log, never payloads.
  - **Secrets**: like distributed agents, a test needing `{{secret.X}}` is refused unless the user
    explicitly allows sending the resolved values (`--allow-secrets-to-cloud` / a checkbox); the
    edge keeps them in memory for the run only.
  Local load testing stays free and unlimited: `devall load run` on your own machines or CI runners,
  and distributed load agents on your own infrastructure.

## Hosted mocks: supported WireMock subset

The edge implements this subset (machine-readable in `worker/src/hosted-mock-subset.json`, which
the desktop's and the CLI's publish check also read, so a stub is flagged *before* upload):

- **Request matching**: `method` (or `ANY`), `url`, `urlPath`, `urlPattern`, `urlPathPattern`,
  `urlPathTemplate` (`/users/{id}`); `queryParameters`, `headers` (case-insensitive names),
  `cookies`, `formParameters` with `equalTo` (+ `caseInsensitive`), `contains`, `doesNotContain`,
  `matches`, `doesNotMatch`, `absent`, `anything`, `and`, `or`, `not`; `bodyPatterns` with those
  plus `equalToJson` (string or object; `ignoreArrayOrder`, `ignoreExtraElements`, json-unit
  placeholders `${json-unit.ignore}` / `any-string` / `any-number` / `any-boolean` / `regex`),
  `matchesJsonPath` (expression, or `{expression, <matcher>}`), `binaryEqualTo`;
  `basicAuthCredentials`; `host`. Regexes are full-match like Java's (`(?i)` prefix supported;
  Java-only syntax such as possessive quantifiers isn't).
- **JSONPath**: `$`, `.name`, `['name']`, `[n]` (negative too), `[a,b]`, `[start:end]`, `*`,
  `..name`, filters `[?(@.x)]`, `[?(@.x <op> literal)]` (`==`, `!=`, `<`, `<=`, `>`, `>=`,
  `=~ /re/`, `&&`, `||`); objects filter themselves like Jayway. No functions (`length()`).
- **Selection**: `priority` (lower first, default 5), then the most recently added; scenarios
  (`scenarioName`, `requiredScenarioState`, `newScenarioState`, initial `Started`; reset from the
  app or `POST /v1/mocks/<name>/scenarios/reset`).
- **Responses**: `status`, `statusMessage`, `headers` (string or list), `body`, `jsonBody`,
  `base64Body`, `bodyFileName` (uploaded `__files` ≤ 256 KB each), `fixedDelayMilliseconds` and
  `delayDistribution` `fixed` / `uniform` / `lognormal` (capped at 10 s),
  `transformers: ["response-template"]` + `transformerParameters`, or templating for every stub
  (`globalTemplating`). CORS: preflights answered and `access-control-allow-origin` added when
  enabled (default on).
- **Templating** (Handlebars subset, never HTML-escaped like WireMock 3): `{{x}}`, `{{{x}}}`,
  comments, `~` whitespace control, `\{{` literal; blocks `#if` / `#unless` (with `else` and
  `else if`), `#each` (`@index`, `@key`, `@first`, `@last`, `this`), `#with`, and any condition
  helper as a block (`{{#eq a b}}…{{else}}…{{/eq}}`); sub-expressions, hash arguments, `../`.
  Model: `request.url`, `request.path` (+ `.[n]` segments and `.<var>` from `urlPathTemplate`),
  `request.pathSegments`, `request.query.<name>` (+ `.[n]`), `request.headers.<name>`
  (case-insensitive), `request.cookies.<name>`, `request.method`, `request.body`,
  `request.bodyAsBase64`, `request.baseUrl`, `request.host`, `request.port`, `request.scheme`,
  `request.clientIp`, `request.id`, `parameters.*`. Helpers: `jsonPath` (+ `default=`), `now`
  (`format=` Java-style `yyyy MM dd HH mm ss SSS EEE a Z X`, `epoch`, `unix`; `offset='3 days'`,
  months/years too; UTC), `randomValue` (`length`, `type` ALPHANUMERIC / ALPHABETIC / NUMERIC /
  HEXADECIMAL / ALPHANUMERIC_AND_SYMBOLS / UUID, `uppercase`), `randomInt`, `randomDecimal`,
  `random 'Name.firstName'` (faker-lite: Name first/last/full/username, Internet email/safe
  email/url/domain/uuid/ipv4/password, Address city/country/street/zip/lat/long, Company name,
  PhoneNumber, Lorem word/sentence/paragraph, Number digit/random, Color name, Bool),
  `pickRandom`, `array`, `range`, `join`, `size`, `math`, `upper`, `lower`, `capitalize`,
  `capitalizeFirst`, `trim`, `replace`, `substring`, `abbreviate`, `default` /
  `defaultIfEmpty`, `base64` (`decode=true`), `urlEncode` (`decode=true`), `toJson`,
  `formatJson`, `eq` `neq`/`ne` `gt` `gte` `lt` `lte` `and` `or` `not` `contains` `matches`.
  Output is capped at 2 MB and `#each`/`range` at 10 000 iterations.
- **Not available** (the publish check lists every use): `equalToXml`, `matchesXPath`,
  date/time matchers, `hasExactly`/`includes`, `multipartPatterns`, custom matchers, `fault`
  (answers 502 with `x-devall-fault`), `proxyBaseUrl` (501), `chunkedDribbleDelay` (ignored),
  other transformers and extensions, data sources (`mocks/data`), webhooks/serve-event
  listeners, helpers outside the list above (e.g. `xPath`, `parseJson`, `formData`, `assign`,
  `regexExtract`, `date`, `parseDate`) — a stub using one answers 500 with the reason. Requests
  that match nothing get `404` with a short explanation and appear in the journal.

## Owner configuration checklist

Before deploying the hosted relay (everything else works locally and in tests):

- [ ] `relay/worker/wrangler.toml`: `routes` (tunnel zone, plus `*/*` for custom domains),
  `PUBLIC_DOMAIN`, `FIREBASE_PROJECT_ID`, `ABUSE_EMAIL`, `SOURCE_URL`, `UPGRADE_URL`, KV namespace
  id, optional `CUSTOM_DOMAIN_TARGET`, `MOCK_URL_STYLE` / `MOCK_DOMAIN`.
- [ ] Worker secrets: `ADMIN_TOKEN`, `MONITOR_SECRET_KEY` (and `RELAY_TOKENS` for self-host tokens).
- [ ] DNS (proxied, `100::`): `*`, `relay`, `hooks`, `share`, `mock`, `custom`; optional `*.mock` +
  Advanced Certificate for host-style mock URLs.
- [ ] Cloudflare for SaaS enabled on the zone, fallback origin `custom.<domain>`.
- [ ] Firebase: Auth providers, `hosting/config.js`, secrets `STRIPE_WEBHOOK_SECRET` and
  `CF_API_TOKEN` (KV edit + SSL and Certificates edit), params `CF_ACCOUNT_ID`,
  `CF_KV_NAMESPACE_ID`, `CF_ZONE_ID`, `PUBLIC_DOMAIN`, `CUSTOM_DOMAIN_TARGET`; Blaze plan (Cloud
  Scheduler for `refreshPendingCustomDomains`).
- [ ] Stripe product, Payment Link, customer portal, webhook endpoint (section 3).
- [ ] `crates/devall-relay/src/hosted.rs`: `HOSTED_ENDPOINT`, `FIREBASE_API_KEY`,
  `FIREBASE_PROJECT_ID`, `SIGNIN_URL`, `UPGRADE_URL`, `FUNCTIONS_URL`.
- [ ] Public Suffix List submission (section 5).

## Development

```sh
cd relay/worker && npm install && npx tsc --noEmit && npx vitest run
cd relay/firebase/functions && npm install && npx tsc --noEmit && npx vitest run
cargo test -p devall-relay -p devall-cli -p devall-load
node relay/e2e/run.mjs   # live: wrangler dev --local + the real CLI, every scenario (e2e/README.md)
npx wrangler dev         # local edge on http://127.0.0.1:8787 (use Host headers relay./hooks./<sub>.)
```

Under `wrangler dev`, the bare loopback host (`localhost`, `127.0.0.1`, `[::1]`) serves the relay
routes, so clients that can't set a `Host` header (the CLI, the app) work against
`ws://127.0.0.1:8787`; public hosts (`<sub>.<domain>`, `hooks.`, `share.`, `mock.`) are reached
with a `Host` header. Cloudflare never routes those loopback names to a deployed Worker.

Worker tests run inside workerd through `@cloudflare/vitest-plugin` (the successor of
`@cloudflare/vitest-pool-workers`): real Durable Objects, SQLite storage, KV and hibernatable
WebSockets, with a mocked desktop client speaking the protocol. Nothing contacts Cloudflare,
Google or Stripe in tests.

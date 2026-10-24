# Relay end-to-end checks (live, local)

`run.mjs` runs the real relay Worker under `wrangler dev --local` — workerd with real Durable
Objects, SQLite storage, KV, alarms and hibernatable WebSockets — in self-host mode and drives it
with the **real Rust CLI** (`target/debug/devall`) plus plain HTTP / WebSocket clients written in
the script. Nothing is deployed and nothing leaves the machine: no `wrangler login`, no
Cloudflare/Firebase/Stripe/Google calls (wrangler telemetry and Miniflare's `cf.json` fetch are
switched off; wrangler's own npm update check is the only outbound request).

```sh
cd relay/worker && npm install          # once (wrangler)
cargo build -p devall-cli               # built automatically when target/debug/devall is missing
node relay/e2e/run.mjs                  # every scenario, ~2 min (80 s of it is the idle check)
node relay/e2e/run.mjs tunnel inbox     # only scenarios whose name contains a word
KEEP_TMP=1 node relay/e2e/run.mjs       # keep the temp folder (wrangler.log, projects, state)
```

It prints `PASS`/`FAIL` per scenario and exits non-zero when anything failed. Every process it
starts (wrangler + workerd, the CLI tunnels/consumers/listeners) and every local server is
stopped at the end, also on failure or Ctrl+C (`taskkill /T` on Windows, process groups
elsewhere); the temp folder is removed once workerd has released its files.

## Setup it creates

- A generated `wrangler.e2e.toml` (the real `wrangler.toml` without `routes` — routes would make
  wrangler rewrite every URL to the zone — and an absolute `main`), a fresh `--persist-to` state
  folder and vars: `PUBLIC_DOMAIN=localhost.test`, `AUTH_MODE=static`, three static tokens
  (`e2e`, `other` → `selfhost`, `free` → the free plan via `STATIC_PLANS`, with an 8 s session via
  `LIMITS_JSON`), `ADMIN_TOKEN`, `MONITOR_SECRET_KEY`, `CUSTOM_HOSTS=api.customer.test=e2e-http`,
  `MOCK_URL_STYLE=path`, `LOAD_COLO_PROBE=off`, `LOAD_VERIFY_DNS=off`.
- Routing by `Host` header: public hosts (`<sub>.localhost.test`, `hooks.`, `share.`, `mock.`,
  `api.customer.test`) are requested at `127.0.0.1:<port>` with that `Host`. The CLI talks to
  `ws://127.0.0.1:<port>` — the Worker serves the relay routes on the bare loopback host.
- A local app (HTTP + WebSocket echo, SSE, gzip, health, alert receiver, verification file) and a
  TCP echo server that greets first.
- Temp projects created with `devall init` for the inbox, mock and load scenarios.

## Scenarios

1. **HTTP tunnel** — `devall tunnel http --subdomain e2e-http --json`: status, body, headers
   (`X-Forwarded-For/Proto/Host/Port`, public `Host` kept, no `x-devall-*`/`cf-*` leaking),
   300 KiB and chunked uploads byte-for-byte, an 8 MiB streamed response (flow control), SSE
   arriving incrementally, `Content-Encoding: gzip` untouched, several `Set-Cookie`, 204/HEAD/302,
   30 concurrent requests, WebSocket pass-through (subprotocol, text, 100 KiB binary), and the
   CLI's request log.
2. **TCP tunnel** — `devall tunnel tcp` ← `devall tcp connect` (owner account, and the access
   secret) ← a client: the server's greeting arrives first, 4 MiB echoed byte-for-byte, a wrong
   secret is refused with the relay's reason.
3. **Hosted inbox** — webhooks POSTed while nobody is connected are buffered; a protocol-level
   client gets them in order and byte-exact, acknowledges one; after a reconnect exactly the
   unacknowledged ones come again; `devall inbox listen --json` stores them in the project's
   webhook store, takes a live one, and a restart redelivers nothing.
4. **Hosted mock** — `devall mock publish` (templating, a scenario, `__files`), requests on the
   path-style mock host, `devall mock journal`, `devall mock unpublish` → 404.
5. **Shares** — `POST /v1/shares` with a desktop-style redacted snapshot, the share page (200,
   shows the request, no secret, CSP), `raw.json`, revoke → 410.
6. **Monitors** — a monitor with an uploaded secret header against the local app; the cron is
   fired for three consecutive 5-minute slots: pass → fail (down alert to the local listener) →
   pass (up alert); the secret reached the app but never the results.
7. **Custom domain** — `Host: api.customer.test` reaches the `e2e-http` tunnel (and HELLO lists
   it); an unmapped host gets 404.
8. **Cloud load** — `devall load cloud verify` is refused without the token file, passes once the
   app serves it; `devall load cloud run --regions weur,enam` (4/s × 5 s) → the merged summary has
   20 requests, 10 per region, thresholds evaluated, saved under `load/runs/`; a 12 s ramp from
   three regions crosses a segment boundary and still adds up to exactly 180; the admin kill
   switch refuses runs.
9. **Free tier** — a random `adjective-noun-NNNN` name, the browser interstitial (API clients pass,
   the app isn't hit), the one-tunnel limit with its message, and session expiry → transparent
   reconnect to a new random URL (old one offline).
10. **Idle heartbeat** — the scenario-1 tunnel stays connected > 75 s idle (three heartbeat
    periods) without reconnecting and still serves: pings are answered by the WebSocket
    auto-response while the Durable Object hibernates.
11. **Relay restart** — wrangler is killed and restarted on the same port and state: the tunnel
    reconnects to the same URL and the inbox listener to the same inbox, and traffic flows again.

## What local wrangler can't show

- **Regions**: `locationHint` is ignored locally (every LoadDO runs in the one workerd process)
  and the colo probe is off, so `colo` is `null`; the summary math, segmenting and the
  verification flow are the same as on Cloudflare.
- **DNS TXT verification** uses DNS-over-HTTPS to 1.1.1.1 — switched off here
  (`LOAD_VERIFY_DNS=off`); covered by the Worker unit tests with a faked resolver.
- **Cron**: `wrangler dev --test-scheduled`'s `/__scheduled` ignores `?time=` in wrangler 4.151
  (the Worker gets "now"), so the script fires the scheduled handler through wrangler's Local
  Explorer API (`POST /cdn-cgi/local/explorer/api/local/scheduled?worker=…` with
  `scheduled_time`) to pick exact 5-minute slots.
- **TLS, Cloudflare for SaaS, Firebase ID tokens, rate-limit bindings**: self-host mode over plain
  `ws://`/`http://`; those paths are covered by unit tests (`relay/worker/test`,
  `crates/devall-relay/tests`).
- **Ctrl+C** of the CLI on Windows: child processes can't be sent a console Ctrl+C, so they are
  terminated; on macOS/Linux the inbox listener is stopped with SIGINT.

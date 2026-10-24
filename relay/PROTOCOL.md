# /dev/all relay wire protocol — version 1

This is the contract between the desktop client (`crates/devall-relay`) and the relay edge
(`relay/worker`). Both sides implement exactly what is written here; change this file first.

- **Transport:** one WebSocket per tunnel and one per webhook inbox, over TLS on port 443
  (`wss://`). It works through corporate proxies and firewalls that only allow HTTPS, and it is
  the only transport Cloudflare Durable Objects can terminate.
- **Multiplexing:** many public HTTP requests and WebSockets share one client socket as numbered
  *streams* with per-stream credit (flow control).
- **Byte exactness:** request and response bodies are forwarded byte-for-byte (never decoded,
  re-encoded or decompressed). The exact public URL is sent with every request and inbox event,
  so provider signatures that cover the URL (Twilio) can be verified locally.

## 1. Endpoints

| Endpoint | Purpose |
|---|---|
| `wss://relay.<domain>/connect?name=<sub>` | Tunnel client socket. `name` is optional (see §3). `&kind=tcp` publishes a raw TCP tunnel instead of an HTTP one (§2.6; Supporter / self-host). |
| `wss://relay.<domain>/tcp?name=<sub>` | TCP consumer socket: one bridged TCP connection through the TCP tunnel `<sub>` (§2.6). |
| `wss://relay.<domain>/inbox?project=<key>` | Webhook-inbox client socket. `project` is an opaque key (`[A-Za-z0-9_-]{1,64}`) so Supporters get one stable inbox per project; ignored on the free tier (one inbox per account). |
| `GET https://relay.<domain>/v1/account` | Account, plan, limits, usage, reserved subdomains, inboxes (JSON). |
| `https://<sub>.<domain>/…` | Public side of a tunnel. |
| `https://<custom hostname>/…` | Public side of a tunnel through a custom domain (§3.1). |
| `https://hooks.<domain>/<inbox-id>/<path…>` | Public side of a webhook inbox (captures any method). |
| `https://<name>.<mock domain>/…`, `https://mock.<domain>/<name>/…` | A published (hosted) mock (§6.1). |
| `https://share.<domain>/<id>` | A shared request/response snapshot (§6.2). |
| `https://relay.<domain>/v1/mocks…`, `/v1/shares…`, `/v1/monitors…`, `/v1/load…`, `/v1/domains` | Account API for the hosted extras (§6). |

### 1.1 Handshake

The client opens the WebSocket with:

```
GET /connect?name=brave-otter-4821 HTTP/1.1
Host: relay.devall.dev
Upgrade: websocket
Authorization: Bearer <Firebase ID token | self-host static token>
Sec-WebSocket-Protocol: devall-relay.1
User-Agent: devall-relay/0.1.0
```

- **Authentication** happens before the upgrade. Hosted relays accept a Firebase ID token
  (RS256, verified against Google's `securetoken@system.gserviceaccount.com` JWKS, `aud` = project
  id, `iss` = `https://securetoken.google.com/<project>`, `exp`/`iat`/`auth_time` checked).
  Self-hosted relays accept a static token from the `RELAY_TOKENS` secret. A relay may accept both.
- **Version negotiation:** the client lists the versions it speaks in `Sec-WebSocket-Protocol`
  (`devall-relay.<n>`, highest first). The server answers with the one it picked; HELLO repeats it.
  No common version → HTTP 426 with `{"error":{"code":"unsupported_version","supported":[1]}}`.
- **Errors before the upgrade** are plain HTTP responses with a JSON body
  `{"error":{"code":"…","message":"…"}}` (clients read the whole body — it may arrive after the
  response head — and show `message`):

| Status | Code | Client behaviour |
|---|---|---|
| 401 | `unauthorized` | refresh the ID token once and retry; then stop and ask the user to sign in |
| 403 | `forbidden`, `subdomain_not_reserved`, `plan_required` | stop, show message |
| 404 | `unknown_inbox` | stop |
| 409 | `subdomain_in_use` | stop (another account holds the name) |
| 426 | `unsupported_version` | stop, ask the user to update |
| 429 | `rate_limited`, `too_many_tunnels`, `quota_exceeded` | back off (`Retry-After` header) |
| 451 | `blocked` | stop (abuse takedown) |

## 2. Frames

Every protocol message is a **binary** WebSocket message:

```
 0        1                5
 +--------+----------------+----------------------------- … --+
 |  type  | stream id (BE) | payload                           |
 |   u8   |      u32       | (rest of the message)             |
 +--------+----------------+----------------------------- … --+
```

Payload encodings: **json** (one UTF-8 JSON object), **bytes** (raw), **u32** (4 bytes,
big-endian), **empty**. Stream `0` is the connection itself. Unknown frame types are ignored
(forward compatibility); a malformed frame closes the socket with code 4000.

**Text** WebSocket messages are reserved for the heartbeat: the client sends the exact text
`devall-relay:ping` every `heartbeatSeconds`, the server answers `devall-relay:pong`. On
Cloudflare this is a `setWebSocketAutoResponse` pair, answered at the edge **without waking the
Durable Object**, so an idle tunnel costs nothing. (The auto-response applies to every socket of
the object, including public pass-through sockets, hence the unusual strings.) Three missed pongs →
the client reconnects.

### 2.1 Connection frames (stream 0)

| Code | Name | Dir | Payload |
|---|---|---|---|
| `0x01` | HELLO | S→C | json, §2.4 — sent once, right after the upgrade |
| `0x02` | ERROR | both | json `{code, message, fatal?}` — `fatal: true` means the socket closes next |
| `0x03` | QUOTA | S→C | json `{kind, used, limit, resetAt?, message}` — `kind`: `bandwidth`, `session`, `inbox_daily`, `inbox_buffer`, `rate` |
| `0x04` | PING | both | bytes (≤ 64), answered with PONG carrying the same bytes (RTT measurement; wakes the DO, so not used as heartbeat) |
| `0x05` | PONG | both | bytes |
| `0x06` | GOAWAY | S→C | json `{reason, reconnect, retryAfterMs?, message?}` — `reason`: `session_expired`, `replaced`, `blocked`, `server_restart`, `quota` |
| `0x07` | USAGE | S→C | json `{bandwidthBytes, requests, periodStart, periodEnd}` — sent after usage is flushed (≥ 1 MiB or ≥ 30 s) |
| `0x08` | TUNNEL_CONFIG | C→S | json `{offlineInbox?: {inboxId, pathPrefixes: [string]} \| null, tcp?: {secretSha256: string \| null}}` — `offlineInbox`: buffer matching requests into the account's inbox while the tunnel is offline; `tcp` (TCP tunnels): SHA-256 (64 lower-case hex) of the tunnel's access secret, `null` = only the owner's account may connect. Not kept across connections: send it after every HELLO. |

### 2.2 HTTP streams (stream ≥ 1, ids chosen by the server)

| Code | Name | Dir | Payload |
|---|---|---|---|
| `0x10` | OPEN | S→C | json request head (below) |
| `0x11` | DATA | S→C | bytes — request body chunk (≤ `maxFrame`) |
| `0x12` | END | S→C | empty — request body complete (always sent, also for empty bodies) |
| `0x13` | RESPONSE | C→S | json `{status, headers}` |
| `0x14` | RESPONSE_DATA | C→S | bytes — response body chunk (≤ `maxFrame`) |
| `0x15` | RESPONSE_END | C→S | empty |
| `0x16` | RESET | both | json `{code, message}` — abort the stream (`timeout`, `cancelled`, `too_large`, `local_unreachable`, `protocol_error`, `quota`, `internal`) |
| `0x17` | WINDOW | both | u32 — credit increment for the stream's body in the other direction |

Request head (OPEN):

```json
{
  "method": "POST",
  "url": "https://brave-otter-4821.devall.dev/webhooks/twilio?x=1",
  "path": "/webhooks/twilio?x=1",
  "headers": [["content-type", "application/x-www-form-urlencoded"], ["x-twilio-signature", "…"]],
  "clientIp": "203.0.113.7",
  "bodyLength": 312
}
```

- `url` is the exact public URL the caller used (scheme, host, path, query — not re-encoded).
- `headers` is an ordered list of `[name, value]` pairs as exposed by the edge runtime
  (lower-case names; Workers sorts them and joins repeated headers except `set-cookie`). Edge
  headers (`cf-*`, `cdn-loop`, `x-forwarded-*`, `x-real-ip`) and the relay's own `x-devall-*`
  headers are removed.
- `bodyLength` is the request's `Content-Length`, or `null` when the body is chunked/unknown.
- The client adds `X-Forwarded-For` (appending `clientIp`), `X-Forwarded-Proto: https`,
  `X-Forwarded-Host` (public host) and `X-Forwarded-Port: 443` before calling the local target,
  removes hop-by-hop headers (`connection`, `keep-alive`, `proxy-connection`, `transfer-encoding`,
  `te`, `trailer`, `upgrade`), and by default keeps the public `Host` (like ngrok, so absolute URL
  generation and Twilio validation work; the client can rewrite it to the local address instead).

Response head (RESPONSE): `{"status": 200, "headers": [["content-type", "text/html"], …]}`.
`status` 100–599 except 101. The server drops hop-by-hop headers and `content-length`, and passes
`content-encoding` through untouched (the body is already encoded; the edge must not re-encode it).

**Flow control.** Each direction of each HTTP stream starts with `window` bytes of credit (from
HELLO, default 262 144). A sender may only send DATA / RESPONSE_DATA while it has credit and
subtracts each chunk's length. A receiver returns credit with WINDOW once the bytes have been
consumed (written to the local server / read by the public client), batching updates of at least
`window / 2`. `window ≥ maxFrame`, so a full frame always fits. Credit is per stream; the socket
itself is not flow-controlled.

**Lifecycle.** OPEN → DATA* → END from the server; RESPONSE → RESPONSE_DATA* → RESPONSE_END from
the client. The client may answer before END (e.g. an early 413). If no RESPONSE arrives within
`requestTimeoutMs`, the server sends RESET `timeout` and answers 504. A request body larger than
`maxRequestBodyBytes` is answered 413 by the edge (by `Content-Length` before OPEN, or with RESET
`too_large` mid-stream). RESET ends a stream in both directions; later frames for that stream are
ignored. If the local target is unreachable the client answers a normal `502` RESPONSE with a short
plain-text explanation (so the public caller sees why).

### 2.3 WebSocket pass-through (stream ≥ 1)

| Code | Name | Dir | Payload |
|---|---|---|---|
| `0x20` | WS_OPEN | S→C | json `{url, path, headers, protocols, clientIp}` — a public client asked to upgrade |
| `0x21` | WS_ACCEPT | C→S | json `{protocol?}` — the local server accepted; `protocol` is the negotiated subprotocol |
| `0x22` | WS_REJECT | C→S | json `{status, message}` — the local server refused (or was unreachable) |
| `0x23` | WS_TEXT | both | bytes (UTF-8 text message) |
| `0x24` | WS_BINARY | both | bytes (binary message) |
| `0x25` | WS_CLOSE | both | json `{code, reason}` |

The edge completes the public upgrade (101) only after WS_ACCEPT, so the public client sees the
local server's subprotocol choice or its rejection status. Messages have no credit window; a
message larger than `maxWsMessage` closes that stream with 1009. Public sockets are hibernatable
on the edge, so idle pass-through sockets cost nothing either.

### 2.4 HELLO

```json
{
  "version": 1,
  "kind": "tunnel",
  "sessionId": "s_01JD3S8Q…",
  "publicUrl": "https://brave-otter-4821.devall.dev",
  "publicUrls": ["https://brave-otter-4821.devall.dev"],
  "subdomain": "brave-otter-4821",
  "inboxId": null,
  "plan": "free",
  "limits": {
    "sessionSeconds": 28800,
    "bandwidthBytesPerMonth": 1073741824,
    "maxRequestBodyBytes": 10485760,
    "requestTimeoutMs": 60000,
    "maxTunnels": 1,
    "interstitial": true,
    "inboxRetentionSeconds": 86400,
    "inboxEventsPerDay": 500,
    "inboxMaxBuffered": 500,
    "inboxMaxBodyBytes": 1048576,
    "maxInboxes": 1,
    "reservedSubdomains": 0,
    "ipRequestsPerMinute": 300
  },
  "usage": {"bandwidthBytes": 52428800, "requests": 1200, "periodStart": "2026-10-01T00:00:00.000Z", "periodEnd": "2026-11-01T00:00:00.000Z"},
  "sessionExpiresAt": "2026-10-22T00:13:24.517Z",
  "window": 262144,
  "maxFrame": 65536,
  "maxWsMessage": 1048576,
  "heartbeatSeconds": 25,
  "serverTime": "2026-10-21T16:13:24.517Z"
}
```

`null` limits mean "unlimited" (self-hosted relays). For `kind: "inbox"`, `publicUrl` is
`https://hooks.<domain>/<inbox-id>/` and `subdomain` is `null`. `kind: "tcp"` is a raw TCP tunnel
(§2.6). `publicUrls` starts with `publicUrl` and adds every custom domain that serves the tunnel
(§3.1).

Limits added in Batch D (additive; older clients ignore them, newer clients default missing ones):

```json
{
  "customDomains": 2, "tcpTunnels": true, "tcpStreamsPerTunnel": 20,
  "hostedMocks": 10, "mockMaxStubs": 1000, "mockMaxBytes": 20971520, "mockRequestsPerDay": 100000, "mockJournalSize": 500,
  "shares": 500, "shareMaxBytes": 5242880, "shareTtlSeconds": 7776000,
  "monitors": 20, "monitorMinIntervalMinutes": 5, "monitorMaxRequests": 20, "monitorHistory": 2016
}
```

Cloud load-generator regions (additive; missing = not offered): `{"loadRegions": 3,
"loadMaxDurationSeconds": 60, "loadMaxRps": 100, "loadRunsPerDay": 20, "loadPrivateTargets":
false}` (`null` = unlimited; free plans have `loadRegions: 0` and `monitors: 0`).

### 2.5 Webhook inbox (stream 0)

| Code | Name | Dir | Payload |
|---|---|---|---|
| `0x30` | INBOX_EVENT | S→C | json event (below) |
| `0x31` | INBOX_ACK | C→S | json `{ids: [string]}` |
| `0x32` | INBOX_STATUS | S→C | json `{buffered, inFlight, todayCount, dailyLimit, rejectedToday, retentionSeconds, oldestReceivedAt, response}` |
| `0x33` | INBOX_CONFIG | C→S | json `{response: {status, headers, body}}` — what the inbox answers to webhook senders |

Every captured request is stored first (Durable Object SQLite) and answered immediately with the
inbox's configured response (default `200 {"ok":true}`, `content-type: application/json`; HTML
content types are downgraded to `text/plain` with `Content-Security-Policy: sandbox` so inboxes can't
serve pages). The relay pushes stored events **in receive order**, at most 100 unacknowledged at a
time; the client acknowledges each event *after* its local handler succeeded; acknowledged events
are deleted from the relay. Unacknowledged events are redelivered after a reconnect, so delivery is
**at-least-once** and the client dedupes by `id`.

Event (`INBOX_EVENT`) — this is also the shape handed to the desktop's webhook module
(`devall-webhooks` `ingest`):

```json
{
  "id": "evt_01JD3SB2W4Q8ZC5V7N2K6M0R1T",
  "seq": 42,
  "inboxId": "k7r2m9x4q1w8e5t3",
  "method": "POST",
  "url": "https://hooks.devall.dev/k7r2m9x4q1w8e5t3/stripe?source=cli",
  "path": "/stripe?source=cli",
  "headers": [["content-type", "application/json"], ["stripe-signature", "t=1729527204,v1=…"]],
  "bodyBase64": "eyJpZCI6ImV2dF8xIn0=",
  "bodySize": 15,
  "receivedAt": "2026-10-21T16:13:24.517Z",
  "sourceIp": "54.187.174.169"
}
```

`path` is the part after the inbox id (with query). `url` is the exact public URL. `bodyBase64` is
the raw body, byte-for-byte.

Limits (from HELLO): events older than `inboxRetentionSeconds` are purged; past
`inboxEventsPerDay` (UTC day) or `inboxMaxBuffered` the inbox answers `429` with `Retry-After`
so the sender retries later; bodies above `inboxMaxBodyBytes` get `413`.

### 2.6 Raw TCP tunnels (TCP over WebSocket)

Workers can't accept inbound TCP, so a TCP tunnel works like `cloudflared access tcp`: both ends
speak WebSockets on 443 and the relay bridges them.

- **Publisher** (`devall tunnel tcp --port 5432`, or the desktop): `wss://relay.<d>/connect?kind=tcp[&name=<sub>]`.
  HELLO has `kind: "tcp"`. Free accounts get `403 plan_required` on the hosted relay; self-hosted
  relays allow it for every token. Public HTTP requests to `https://<sub>.<d>` get a `404` page
  explaining how to connect.
- **Consumer** (`devall tcp connect <sub> --local-port 15432`): listens locally; for **each** accepted
  TCP connection it opens `wss://relay.<d>/tcp?name=<sub>` with either
  - `Authorization: Bearer <ID token | self-host token>` of the **same account** as the publisher, or
  - `Authorization: Bearer dtcp_<secret>`, the tunnel's access secret (the publisher generates it and
    sends only its SHA-256 in TUNNEL_CONFIG; the edge hashes what the consumer presents).

  There is no anonymous access. The upgrade completes only after the publisher connected to its
  local port, so failures are HTTP errors: `404 unknown_tunnel` (no TCP tunnel online under that
  name), `403 forbidden` (wrong account/secret), `502 local_unreachable` (publisher refused),
  `504 timeout` (no answer in 15 s), `429 too_many_connections` / `rate_limited` / `quota_exceeded`.

| Code | Name | Dir | Payload |
|---|---|---|---|
| `0x40` | TCP_OPEN | S→C (publisher) | json `{clientIp, via: "owner" \| "secret"}`: a consumer wants a connection; open one to the local target |
| `0x41` | TCP_ACCEPT | C→S (publisher) | empty: the local connection is up; the relay completes the consumer's upgrade |
| `0x42` | TCP_DATA | both | bytes (≤ `maxFrame`), only while the sender has credit |
| `0x43` | TCP_CLOSE | both | json `{reason, message?}`, `reason`: `eof`, `reset`, `local_unreachable`, `timeout`, `offline`, `protocol_error`. Ends the stream in **both** directions (no half-close). |
| `0x17` | WINDOW | both | u32: credit for TCP_DATA in the other direction (same rules as §2.2) |

On the **consumer socket** the same frames are used with stream id **1**; the relay rewrites ids
in both directions and forwards TCP_DATA / WINDOW / TCP_CLOSE untouched, so credit is end to end
(consumer ↔ publisher) and the relay never buffers stream data (frames that arrive before the
consumer's upgrade completed, e.g. a server greeting, are held until it does). Each direction
starts with the default window (262 144 bytes) and `maxFrame` 65 536. The consumer socket answers
the text heartbeat like any client socket, so idle database sessions hibernate. Closing the
consumer socket sends TCP_CLOSE `eof` to the publisher; TCP_CLOSE from the publisher closes the
consumer socket (1000); a publisher that goes away closes its consumers with 1001. TCP bytes count
towards the tunnel's bandwidth quota.

## 3. Subdomains and sessions

- **Free:** a random `adjective-noun-NNNN` name per session (8 h). Reconnecting after a network
  drop with `name=<the same name>` resumes the session (same URL, same expiry) when the account
  matches and the session hasn't expired. Random names are not handed to another account within
  an hour of their last use.
- **Supporter:** `name` must be one of the account's reserved subdomains (claimed through the
  `claimSubdomain` function); without `name` a random one is assigned. Sessions last 24 h, then
  GOAWAY `session_expired` with `reconnect: true` makes the client reconnect with a fresh token
  (this is also how plan changes reach long-lived sockets).
- **Self-hosted:** any valid, non-reserved name.
- A second connection for the same subdomain from the same account replaces the first (the old
  socket gets GOAWAY `replaced`, `reconnect: false`, close code 4009).

### 3.1 Custom domains

A request whose `Host` is outside the relay's domain is looked up as a custom domain:

1. **Self-hosted:** the `CUSTOM_HOSTS` map (`{"api.example.com":"staging-box"}`), plus a Worker
   route or Custom Domain for that hostname in the operator's Cloudflare account.
2. **Hosted:** KV `host:<hostname>` → `{sub, uid}`, written by the `addCustomDomain` function (a Cloudflare
   for SaaS custom hostname; the customer CNAMEs the hostname to `CUSTOM_DOMAIN_TARGET`, default
   `custom.<domain>`, and the certificate is issued automatically). The request reaches the
   tunnel only if the tunnel's current owner is `uid`; otherwise it gets the offline page.

Unknown hosts get `404`. The tunnel sees the exact public URL (`https://api.example.com/…`) in OPEN /
WS_OPEN. Status and the DNS records to add are in `/v1/account` → `customDomains` (and `/v1/domains`).

## 4. Close codes

| Code | Meaning |
|---|---|
| 1000 | normal |
| 1001 / 1012 | server going away / restarting (deploy) — reconnect with backoff |
| 4000 | protocol error |
| 4008 | session expired — reconnect |
| 4009 | replaced by a newer connection — don't reconnect |
| 4029 | quota exceeded |
| 4051 | blocked |

## 5. Versioning

- Additive changes (new optional JSON fields, new frame types that may be ignored) keep the
  version number. Receivers ignore unknown JSON fields and unknown frame types. Batch D (TCP
  frames `0x40`–`0x43`, `kind=tcp`, `TUNNEL_CONFIG.tcp`, the new limits and the HTTP endpoints) is
  additive within version 1: an old client never asks for `kind=tcp`, and an old relay answers a
  `kind=tcp` publisher with an HTTP tunnel's HELLO (`kind: "tunnel"`), which new clients detect
  and stop on.
- Anything else bumps the version: the server keeps accepting the previous version for at least
  six months, and HELLO's `version` tells the client which semantics apply.

## 6. Account API for the hosted extras (HTTP, JSON)

All endpoints are on `https://relay.<domain>` and need `Authorization: Bearer <ID token | self-host
token>` (nothing is anonymous). Errors use the `{"error":{"code","message"}}` shape of §1.1 (`401`
unauthorized, `403 plan_required`, `404 not_found`, `409` name taken, `413` too large, `429` over a
count / rate limit).

### 6.1 Hosted mocks

| Method + path | Body / answer |
|---|---|
| `GET /v1/mocks` | `{mocks: [{name, url, hostUrl, pathUrl, stubs, bytes, createdAt, updatedAt, requestsToday, settings}], limits}` |
| `PUT /v1/mocks/<name>` | `{mappings: [WireMock mapping JSON…], files: {"<path under __files>": "<base64>"}, settings?: {cors, globalTemplating}}` → `{name, url, hostUrl, pathUrl, stubs, bytes, files, warnings: [{stub, feature, message}]}`. A publish replaces the whole mock (stubs, files, scenario states). `<name>`: 3–40 chars `[a-z0-9-]`, not reserved, unique across the relay. |
| `GET /v1/mocks/<name>` | info |
| `DELETE /v1/mocks/<name>` | unpublish (storage deleted) |
| `GET /v1/mocks/<name>/requests?limit=50` | `{requests: [{id, at, method, url, headers, body, bodySize, clientIp, matched, stubId, stubName, status, durationMs}]}`, newest first |
| `DELETE /v1/mocks/<name>/requests` | clear the journal |
| `POST /v1/mocks/<name>/scenarios/reset` | every scenario back to `Started` |

The supported WireMock subset is listed in `relay/README.md` ("Hosted mocks").

### 6.2 Shared snapshots

| Method + path | Body / answer |
|---|---|
| `POST /v1/shares` | `{snapshot: {version: 1, title, request: {method, url, headers, body?}, response?: {status, statusText?, headers, body? \| bodyBase64?, bodyTruncated?, size?, contentType?}, error?, timing?: {totalMs}, environment?, at?, curl?, reqYaml?, redactions?: [string]}, ttlSeconds?}` → `201 {id, url, createdAt, expiresAt, bytes}` |
| `GET /v1/shares` | `{shares: [{id, title, url, createdAt, expiresAt, bytes}], limits}` |
| `DELETE /v1/shares/<id>` | revoke |

`https://share.<domain>/<id>` renders a read-only page; `/<id>/raw.json`, `/<id>/request.req.yaml`
and `/<id>/curl.sh` download. Expired or revoked links answer `410`. The client redacts before
uploading (the relay stores exactly what it receives).

### 6.3 Monitors

| Method + path | Body / answer |
|---|---|
| `GET /v1/monitors` | `{monitors: [view], limits: {monitors, monitorMinIntervalMinutes, monitorMaxRequests, monitorHistory, secretsAvailable}}` |
| `POST /v1/monitors` | `{name, intervalMinutes (5, 10, 15, 30, 60, 180, 360, 720, 1440), requests: [{name, method, url, headers, body?, basicAuth?, timeoutMs?, followRedirects?, assertions: [{assert, name?, path?, op, value?}]}], alerts?: {webhookUrl?, slackWebhookUrl?, onRecovery?}, secrets?: {NAME: value}, paused?}` → `201 view` |
| `GET` / `PUT` / `DELETE /v1/monitors/<id>` | view / replace (in `secrets`, `null` removes a stored value; omitted names are kept) / delete |
| `GET /v1/monitors/<id>/results?limit=50` | `{results: [{at, ok, durationMs, trigger, checks: [{name, method, url, status, durationMs, ok, failures, error}]}]}`, newest first |
| `POST /v1/monitors/<id>/run` | run now (at most every 30 s) → the result |

`url`, header values, `body` and `basicAuth` may contain `{{secret.NAME}}`; values come from the
uploaded secrets (stored encrypted, never returned). `view` = `{id, name, intervalMinutes,
effectiveIntervalMinutes, requests, alerts, secretNames, paused, pausedReason, createdAt,
updatedAt, lastRunAt, lastOk, lastAlertError, uptime, runs, recent: [{at, ok, durationMs}], last}`.
Assertions mirror the project format (`status`, `header`, `jsonPath`, `body`, `responseTime`,
`size`, `contentType` × `eq`, `ne`, `lt`, `lte`, `gt`, `gte`, `contains`, `notContains`,
`startsWith`, `endsWith`, `matches`, `exists`, `notExists`, `isEmpty`, `isNotEmpty`, `isType`,
`oneOf`).

### 6.4 Cloud load-generator regions

Supporter or self-hosted (`limits.loadRegions > 0`, else `403 plan_required`). Nothing is sent to a
host the account hasn't verified (relay/README.md → Abuse policy).

| Method + path | Body / answer |
|---|---|
| `GET /v1/load` | `{enabled, regions: [{id, name}], limits: {loadRegions, loadMaxDurationSeconds, loadMaxRps, loadRunsPerDay, loadPrivateTargets}, runsToday, targets: [target], verifyPath}` (also on the free plan) |
| `POST /v1/load/targets` | `{target: "api.example.com" \| "https://h:8443" \| "http://127.0.0.1:3000"}` → `201 target` (or `200` with the existing token). `403 target_not_allowed` for private / loopback hosts on hosted plans and for the relay's own domain; `429 too_many_targets` past 20. |
| `POST /v1/load/targets/verify` | `{target}` → `200 target` (`status: "verified"`), or `422 verification_failed` with `error.details: [string]` and `error.target`. At most one check per target every 2 s (`429`). |
| `DELETE /v1/load/targets?target=<target>` | forget it |
| `POST /v1/load/runs` | a run plan (below) → the merged result when the run is over (the request lasts as long as the test). `400 invalid_plan` (over a plan limit, bad shape), `403 target_not_verified` / `target_not_allowed`, `409 load_busy` (another run of the account is in progress), `429 load_quota` (runs per day), `503 load_disabled` (kill switch). |
| `POST /v1/load/runs/<runId>/stop` | stop at the next segment boundary (≤ 10 s); the run answers with `aborted: true, abortReason: "stopped"` |

`target` = `{key: "host:port", origin, hostname, port, status: pending|verified|expired, token,
fileUrl, dnsName, verifiedAt, expiresAt, method: http|dns|null, lastError}`. Verification: the file
at `fileUrl` (`<origin>/.well-known/devall-load-verification.txt`) answers `200` without a redirect
and contains `token` (≤ 4 KB), or the TXT record `dnsName` (`_devall-load.<host>`) holds it. A file
verification covers `host:port`, a DNS one every port of `host`; both expire after 30 days.

Run plan (built by `devall-load`'s `cloud` module; requests are resolved on the user's machine):

```json
{
  "runId": "01JD…", "name": "Users API smoke", "regions": ["weur", "enam"],
  "profile": {"startRate": 0, "stages": [{"durationMs": 30000, "target": 40}]},
  "maxInFlight": 100,
  "requests": [{"name": "list", "path": "users/list.req.yaml", "method": "GET", "url": "https://api.example.com/users",
                "headers": [["accept", "application/json"]], "bodyBase64": null, "timeoutMs": 10000, "followRedirects": true}],
  "weights": [1],
  "flow": null,
  "thinkTimeMs": null,
  "checks": [{"type": "status", "expect": ["2xx"]}, {"type": "maxDuration", "ms": 500}, {"type": "bodyContains", "text": "id"}]
}
```

`profile` rates are iterations per second, piecewise linear (a constant rate is one stage at the
same rate); exactly one of `weights` (a weighted mix, one per request) or `flow` (request indexes
run in order every iteration) is set. Regions: `wnam enam sam weur eeur apac oc afr me` (Durable
Object location hints — placement is best-effort). Each region gets `1/n` of the rate and of
`maxInFlight`; iterations that find `maxInFlight` busy are dropped and counted. Every request
carries `x-devall-load: <runId>`.

The answer has the devall-load run summary's fields that the edge can fill — `runId, test,
startedAt, durationMs, plannedDurationMs, aborted, abortReason, openModel (true), totals, latency
(from each iteration's intended start), serviceTime, statusCodes, errorKinds, errorSamples,
requests, bucketSeconds, timeline, warnings` — plus `regions: [{region, name, colo, share, ok,
error, requests, failed, droppedIterations, p50, p95}]` (`colo` = the data center the region ran
in, when detected) and `histogram: {buckets: [[index, count]], count, min, max, sum}` (µs, log-linear:
exact below 128, then 64 sub-buckets per power of two). Thresholds are evaluated by the client.

### 6.5 Custom domains

`GET /v1/domains` → `{target, domains: [{hostname, subdomain, status: pending|active|error|static,
sslStatus, records: [{type, name, value, purpose}], errors, source: hosted|static, updatedAt}]}`.
Hosted custom domains are added and removed through the Firebase callables `addCustomDomain
{hostname, subdomain}`, `removeCustomDomain {hostname}` and `refreshCustomDomain {hostname}`
(`https://<region>-<project>.cloudfunctions.net/<name>`, body `{"data": {…}}`, `Authorization:
Bearer <ID token>`); self-hosted relays use `CUSTOM_HOSTS`.

# Running project mocks in CI

The mock server the app runs (`crates/devall-mock`, WireMock-compatible) also runs headless: from
the `devall` CLI or a small Docker image. Same stubs (`mocks/mappings/**/*.json`), body files
(`mocks/__files/`), data sources (`mocks/data/`) and admin API (`/__admin`, WireMock 3.x JSON).

In the app: **Mock server → Run in CI** shows these commands filled in for the open project
(image name, port, chaos preset, project folder inside the repository).

> The admin API has no authentication. Bind to `0.0.0.0` only inside CI, a container network or a
> network you trust; anyone who can reach the port can read and change stubs.

## CLI

```sh
devall mock check                       # load every mapping and data source; exit 1 on errors
devall mock serve --port 8080           # 127.0.0.1:8080 until Ctrl+C / SIGTERM
devall mock serve --port 8080 --bind 0.0.0.0 --chaos flaky --log json
devall mock record --target https://api.example.com --port 8080   # proxy + record; Ctrl+C saves stubs
devall mock health --port 8080          # exit 0 when /__admin/health answers (health checks)
```

`serve` options:

| Flag | Meaning |
|---|---|
| `--project <dir>` | Project folder (default: the current folder or a parent with `devall.yaml`). A folder without `devall.yaml` works too if it has a `mocks/` folder — mount a plain WireMock layout at `<dir>/mocks`. |
| `--port <n>` | Default 8080. |
| `--bind 127.0.0.1 \| 0.0.0.0` | Interfaces to listen on (default 127.0.0.1). |
| `--chaos <preset\|file>` | `slow` (+800 ms ± 600), `flaky` (10% 5xx), `broken` (5% connection faults), `all`, or a JSON/YAML file with a chaos config (`latencyMs`, `jitterMs`, `errorRate`, `errorStatuses`, `faultRate`, `faults`, `urlPattern`). |
| `--log text \| json \| off` | Request log on stdout: one line per request, or JSON lines (`method`, `url`, `status`, `stubName`, `totalTimeMs`, `fault`, …). Startup messages go to stderr. |
| `--verbose` | List every stub and data source at startup and log mapping reloads. |
| `--global-templating` | Template every response (WireMock's `--global-response-templating`). |
| `--cors` | Permissive CORS on stub responses (WireMock's `--enable-stub-cors`). |
| `--strict` | Refuse to start if any mapping, data file, GraphQL schema or WebSocket / gRPC / MQTT definition fails to load. |
| `--grpc-port <n>` | gRPC listener (h2c, server reflection) for `mocks/grpc`. Default: 50051 when `mocks/grpc` has `.proto` files (a busy default port falls back to a free one; an explicit port must bind). `0` = any free port. |
| `--no-grpc` | Don't start the gRPC listener. |
| `--mqtt-port <n>` | Embedded MQTT broker for `mocks/mqtt`. Default: 1883 when `mocks/mqtt` has behaviour files. `0` = any free port. |
| `--no-mqtt` | Don't start the MQTT broker. |

GraphQL schemas (`mocks/graphql/`) and WebSocket stubs (`mocks/websocket/`) are served on the HTTP
port; the startup banner lists every endpoint and listener, and `--log` also prints WebSocket
frames, gRPC calls and MQTT messages. In Docker, publish the extra ports too
(`-p 8080:8080 -p 50051:50051 -p 1883:1883`).

Mapping files are read at startup. After editing them, call `POST /__admin/mappings/reset` or
restart the server. The server stops cleanly on Ctrl+C, `SIGTERM` (`docker stop`) or
`POST /__admin/shutdown`.

Related commands that are handy next to the mocks:

```sh
devall openapi lint openapi/api.yaml                       # exit 1 on errors (--strict: warnings too)
devall openapi drift --spec openapi/api.yaml --mocks       # stubs vs the spec
devall openapi drift --spec openapi/api.yaml --har traffic.har
devall load run smoke.load.yaml --against-mock             # load-test baseline against the mocks
devall run api.hurl --var baseUrl=http://localhost:8080 --junit junit.xml
```

## Docker

The image is built from the `Dockerfile` at the root of the /dev/all repository (multi-stage: Rust
builder → `debian:bookworm-slim` with only `ca-certificates` and the `devall` binary, non-root
user). Its entrypoint is
`devall mock serve --project /project --bind 0.0.0.0 --port 8080`; arguments after the image name
are appended (later flags win).

```sh
# in the /dev/all repository
docker build -t devall-mock .

# in your project (devall.yaml + mocks/ at the current folder)
docker run --rm -p 8080:8080 -v "$PWD:/project:ro" devall-mock
docker run --rm -p 8080:8080 -v "$PWD:/project:ro" devall-mock --chaos flaky --log json

# PowerShell
docker run --rm -p 8080:8080 -v "${PWD}:/project:ro" devall-mock

# a plain WireMock folder (mappings/ and __files/) works when mounted as /project/mocks
docker run --rm -p 8080:8080 -v "$PWD/wiremock:/project/mocks:ro" devall-mock

# check only
docker run --rm -v "$PWD:/project:ro" --entrypoint devall devall-mock mock check --project /project
```

Release tags (`v*`) publish `ghcr.io/<owner>/devall-mock:<version>` (and `:latest`) through
`.github/workflows/docker.yml`; every push to `main` and every pull request touching the crates
builds and smoke-tests the image without publishing it. The published image is `linux/amd64`
(Apple Silicon runs it under emulation, or `docker build` it locally for arm64).

Mount the project read-only (`:ro`) unless you want admin API edits or recordings to be written
back; the container runs as uid 10001, so a writable bind mount on Linux may also need
`--user "$(id -u):$(id -g)"`.

The image has a `HEALTHCHECK` (`devall mock health --port 8080`).

## GitHub Actions

Service containers start before `actions/checkout`, so a mounted repository would still be empty.
Two ways around it:

**1. Start the mock as a step** (no extra image to maintain):

```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Start the mock server
        run: docker run -d --name mocks -p 8080:8080 -v "$GITHUB_WORKSPACE:/project:ro" ghcr.io/<owner>/devall-mock:1
      - name: Wait until it is healthy
        run: for i in $(seq 1 30); do curl -fsS http://localhost:8080/__admin/health && break; sleep 1; done
      - name: Tests against the mocks
        run: npm test
        env:
          API_BASE_URL: http://localhost:8080
      - name: Mock request log
        if: always()
        run: docker logs mocks
```

**2. A service container with the stubs baked in** — build a tiny image from your project once
(and on every stub change):

```dockerfile
# mocks.Dockerfile in your project
FROM ghcr.io/<owner>/devall-mock:1
COPY --chown=devall devall.yaml /project/
COPY --chown=devall mocks /project/mocks
```

```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    services:
      api:
        image: ghcr.io/<you>/my-api-mocks:latest
        ports: ["8080:8080"]
        options: >-
          --health-cmd "devall mock health --port 8080"
          --health-interval 5s --health-timeout 5s --health-retries 10
    steps:
      - uses: actions/checkout@v4
      - run: npm test
        env:
          API_BASE_URL: http://localhost:8080
```

Without Docker, install the CLI (`cargo install --path crates/devall-cli` from the /dev/all
repository, or a release binary) and run `devall mock serve --port 8080 &` in a step; it stops
with the job.

## docker-compose

```yaml
services:
  mocks:
    image: ghcr.io/<owner>/devall-mock:1   # or a local build: devall-mock
    ports:
      - "8080:8080"
    volumes:
      - ./:/project:ro
    command: ["--chaos", "flaky"]          # optional, appended to `devall mock serve …`
    healthcheck:
      test: ["CMD", "devall", "mock", "health", "--port", "8080"]
      interval: 10s

  app:
    build: .
    environment:
      API_BASE_URL: http://mocks:8080
    depends_on:
      mocks:
        condition: service_healthy
```

## GraphQL, WebSocket, gRPC and MQTT

The same engine serves more than HTTP stubs (all from the project's `mocks/` folder, all on
`devall mock serve` and the app's Mock server page):

| Folder | What | Where it is served |
|---|---|---|
| `mocks/graphql/` | SDL (`*.graphql`, `*.graphqls`, `*.gql`; a `<name>/` folder is merged into one schema) or an introspection result (`*.json`); optional `config.json`: `{"<name>": {"endpoint": "/api/graphql", "seed": 7, "listLength": 3}}` | HTTP port: `/graphql` (schema `schema` or the only one), else `/graphql/<name>` |
| `mocks/websocket/*.json` | message stubs (`request`, `onConnect`, `messages` rules, `periodic`, `close`, `echo`) | HTTP port, `ws://…/<path>` |
| `mocks/grpc/` | `*.proto` files (compiled together, imports resolved from this folder) and stub `*.json` files | separate h2c port (`--grpc-port`, default 50051) with server reflection |
| `mocks/mqtt/*.json` | broker behaviours (`retained`, `onSubscribe`, `periodic`, `rules`) | separate port (`--mqtt-port`, default 1883) |

**GraphQL.** A request to a schema endpoint that no stub matched is validated against the schema
(GraphQL-style `errors` with locations) and answered with data generated from the selection set —
deterministic per seed, shaped by field names (`email`, `createdAt`, `price`, …), with `id`
arguments flowing into `id` fields and `first` / `limit` setting list lengths; introspection works.
To pin data, write an ordinary stub in `mocks/mappings/` with a `graphql` matcher and the
`graphql-automock` transformer — the stub body's `data` overrides generated fields (keyed like the
response, aliases included) and its `errors` are passed through:

```json
{
  "request": { "method": "POST", "urlPath": "/graphql",
               "graphql": { "operationName": "GetUser", "variables": { "id": "1" } } },
  "response": { "status": 200, "transformers": ["graphql-automock"],
                "jsonBody": { "data": { "user": { "name": "Ada Lovelace" } } } }
}
```

`graphql` takes `operationName`, `operationType`, `field` (a root field), `query` and `variables`
(an object = these variables, others allowed; or any matcher). WireMock's GraphQL extension
matcher (`"customMatcher": {"name": "gql-matcher", "parameters": {"query", "variables",
"operationName"}}`) is understood too (queries compared after normalisation).

**WebSocket** stubs:

```json
{
  "name": "Chat",
  "request": { "urlPath": "/ws/chat" },
  "onConnect": [ { "text": "welcome {{request.query.user}}" } ],
  "messages": [
    { "match": "ping", "reply": [ { "text": "pong" } ] },
    { "match": { "matchesJsonPath": "$[?(@.type == 'say')]" }, "broadcast": true,
      "reply": [ { "json": { "said": "{{jsonPath message.text '$.text'}}" }, "delayMs": 100 } ] },
    { "match": "bye", "close": { "code": 1000, "reason": "bye" } }
  ],
  "periodic": [ { "intervalMs": 1000, "count": 5, "message": { "text": "tick" } } ]
}
```

**gRPC** stubs (`*.json` anywhere under `mocks/grpc/`):

```json
{
  "method": "shop.v1.Catalog/GetItem",
  "request": { "body": { "id": "42" }, "metadata": { "x-tenant": { "equalTo": "acme" } } },
  "response": { "message": { "id": "42", "name": "Teapot" }, "trailers": { "x-trace": "t-1" } }
}
```

`response.messages` + `streamIntervalMs` stream several messages; `status` (`"NOT_FOUND"` or `5`)
with `statusMessage` returns an error. Client-streaming calls match on the JSON array of all
request messages. `grpcurl -plaintext localhost:50051 list` works through reflection.

**MQTT** behaviours:

```json
{
  "retained": [ { "topic": "devices/1/state", "payload": "on", "qos": 1 } ],
  "rules": [ { "topic": "rpc/request/+",
               "publish": [ { "topic": "rpc/response/{{topicSegments.[2]}}", "json": { "ok": true } } ] } ]
}
```

Admin API additions (built-in engine only, next to WireMock's `/__admin`):

| Endpoint | |
|---|---|
| `GET /__admin/devall/protocols` | status of all four |
| `GET /__admin/devall/graphql` · `POST /__admin/devall/graphql/execute` | schemas and endpoints · auto-mocked result for `{schema?, query, variables?, operationName?}` |
| `GET /__admin/devall/websocket` · `POST …/websocket/send` · `POST …/websocket/close` | stubs and open connections · push `{connectionId? \| path?, text \| json \| binary}` · close `{connectionId? \| path?, code?, reason?}` |
| `GET /__admin/devall/grpc` · `POST …/grpc/start` `{port?}` · `POST …/grpc/stop` | listener, methods (with example requests), stubs |
| `GET /__admin/devall/mqtt` · `POST …/mqtt/start` `{port?}` · `POST …/mqtt/stop` · `POST …/mqtt/publish` `{topic, payload, qos?, retain?}` | broker, clients, retained messages, behaviours |
| `GET /__admin/devall/messages?protocol=websocket\|grpc\|mqtt&limit=` · `DELETE …/messages` | the message journal (frames, calls, publishes, connection events) |
| `POST /__admin/devall/reload` | reloads mappings and every protocol folder |

## Data sources

Stubs can look rows up in CSV / JSON / SQLite files under `mocks/data/` (see `crates/devall-mock/src/data.rs`):

```json
{
  "request": { "method": "GET", "urlPathTemplate": "/users/{id}" },
  "response": {
    "headers": { "Content-Type": "application/json" },
    "body": "{{#if row}}{\"id\": {{row.id}}, \"name\": \"{{row.name}}\"}{{else}}{\"error\": \"not found\"}{{/if}}",
    "transformerParameters": { "dataSource": { "name": "users", "where": { "id": "{{request.path.id}}" }, "notFoundStatus": 404 } }
  }
}
```

or inline: `{{dataSource 'users' 'id' request.path.id}}`, `{{dataRows 'users' 'role' 'admin' limit=10}}`,
`{{dataCount 'users'}}`. They ship with the project folder, so they work the same in CI and Docker.

## Notes

- Untested here: the Dockerfile and workflow were written without a Docker daemon available;
  they follow the standard multi-stage / `docker/build-push-action` patterns. The workflow's smoke
  test step exercises the image on every build.
- Plain HTTP/1.1 only (no TLS on the mock port); put a TLS-terminating proxy in front if needed.
  gRPC is h2c (plaintext HTTP/2) and MQTT is plain TCP for the same reason.

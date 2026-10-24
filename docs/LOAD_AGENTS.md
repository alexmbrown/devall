# Distributed load agents

One machine can only push so much load before the generator itself becomes the bottleneck (CPU,
ephemeral ports, NIC). `devall load` can split a test across **load agents** you run on your own
machines — spare laptops, CI runners, VMs, Docker hosts, a Kubernetes namespace — and merge the
results into one run summary. It is free and local-first like the rest of `devall load`. (The relay
also offers short, bounded cloud-region bursts against hosts you verified — Supporter or
self-hosted, see `FEATURES.md` → *How cloud load regions stay safe* and `devall load cloud`.)

```
            coordinator (devall load run --agents …  or the app's Load page)
              │ plan (its share)                 ▲ NDJSON: snapshots, then summary + HDR histograms
      ┌───────┴────────┐                         │
  agent :7777      agent :7777   …   ──────── load ────────▶ your system under test
```

* The **coordinator** reads the test and the saved requests, resolves them with the chosen
  environment, splits the arrival rate (or the VUs) by agent weight and sends each agent its share.
* Each **agent** runs its share with the same engine as a local run (open/closed models,
  coordinated-omission-corrected latency) and streams snapshots, then its summary and raw HDR
  histograms (V2 format, deflated).
* The coordinator **adds the histograms** (exact percentiles over every sample, not an average of
  averages), sums counters, evaluates the **thresholds on the merged result**, and saves one summary
  on the coordinator's machine (the project data folder in the app data directory — never in the
  repository) with `agents: [...]` recording who ran what (address, share, machine, requests,
  errors). Add `--out summary.json` to also write it where CI can pick it up as an artifact. Exit
  codes are the usual ones (0 / 99 thresholds failed / 105 aborted / 2 needs `--yes`).

## Running agents

Every agent and the coordinator share a bearer token. Generate one and keep it out of git:

```sh
devall load agent token                # prints a random 256-bit token
export DEVALL_AGENT_TOKEN=…            # on every agent and on the coordinator
devall load agent --listen 0.0.0.0:7777
```

`--token` works too, but command lines are visible to other users of the machine; prefer the
environment variable (`--token-env NAME` picks another one). Tokens shorter than 16 characters are
refused. An agent runs one plan at a time (`409` while busy) and stops its run when the coordinator
disconnects or asks it to (`Ctrl-C` on the coordinator, **Stop** in the app).

Endpoints: `GET /healthz` (no auth, for probes), `GET /v1/info`, `POST /v1/run`, `POST /v1/stop`
(all bearer-authenticated; wrong tokens get `401` after a short delay).

### TLS

Agents speak plain HTTP by default: the token, the plans and any secrets you allowed travel in the
clear. On anything but a trusted network, either

* serve HTTPS directly: `devall load agent --tls-cert cert.pem --tls-key key.pem` (rustls), and
  point the coordinator at `https://host:7777` (add `--agents-ca ca.pem` for a private CA), or
* put a TLS terminator (nginx, Caddy, an ingress, a cloud load balancer per agent) in front of
  each agent and use its `https://` URL.

### Docker

The repo's `Dockerfile` has an `agent` target built from the same `devall` binary:

```sh
docker build --target agent -t devall-agent .
docker run --rm -p 7777:7777 -e DEVALL_AGENT_TOKEN="$DEVALL_AGENT_TOKEN" devall-agent
# TLS: -v "$PWD/certs:/certs:ro" devall-agent --tls-cert /certs/cert.pem --tls-key /certs/key.pem
```

The image runs as a non-root user, exposes 7777 and has a `HEALTHCHECK`
(`devall load agent health`). CI (`.github/workflows/docker.yml`) builds and smoke-tests it and
publishes `ghcr.io/<owner>/devall-agent` on version tags.

### Kubernetes

```sh
devall load agent k8s --image ghcr.io/<owner>/devall-agent:<version> --replicas 4 --namespace load > agents.yaml
kubectl apply -f agents.yaml
```

The generator prints a **Secret** (the token — from `DEVALL_AGENT_TOKEN` if set, otherwise a new
random one; keep `agents.yaml` out of git), a **Deployment** (non-root, read-only root filesystem,
readiness/liveness probes on `/healthz`, CPU/memory requests via `--cpu` / `--memory`) and a
**headless Service**, so DNS returns one address per agent pod:

* coordinator inside the cluster (a Job or a debug pod with the project mounted):
  `devall load run test.load.yaml --agents dns:devall-load-agent.load.svc:7777`
  (`dns:` expands to every pod IP);
* coordinator on your machine: `kubectl -n load port-forward pod/<pod> 17777:7777` per agent
  (or the app's Docker & K8s page), then `--agents 127.0.0.1:17777,127.0.0.1:17778,…`.

## Running a distributed test

CLI:

```sh
export DEVALL_AGENT_TOKEN=…
devall load run checkout.load.yaml --env staging \
  --agents 10.0.0.11:7777,10.0.0.12:7777,10.0.0.13:7777 \
  --agent-weights 2,1,1            # optional: relative shares (default equal)
```

* `--agents-token-env NAME` reads the token from another variable.
* `--agents-ca ca.pem` trusts a private CA for `https://` agents.
* The usual guard rails apply first (`--yes` for high rates against non-local hosts and protected
  environments); every agent is checked (reachable, token accepted, idle, same protocol version)
  before any load is sent.

App: open a load test → **Distribute…** → list the agents (address + weight; saved per device),
enter the token (optionally remembered in the OS keychain — never in project files), **Check
agents**, **Run**. Live results show the merged throughput and the worst agent's latency while the
run is in flight; the final summary (Results and History) has an **Agents** table.

How the load is split:

| Executor | Split |
|---|---|
| `constantArrivalRate`, `rampingArrivalRate` | rates (start and stage targets) × share; `preAllocatedVus`/`maxVus` × share, rounded up |
| `constantVus` | VUs by largest remainder (they add up exactly; an agent left with 0 VUs is an error) |
| `rampingVus` | start VUs by largest remainder, stage targets × share |

Agents wait a short, fixed delay after receiving their plan so they start together; their timeline
buckets are merged by time (counts and rates add up; per-bucket p95/p99 show the worst agent — the
run's percentiles and thresholds always come from the merged histograms). `maxVus` in the summary
is the sum of the agents' peaks.

## Secrets

A plan contains what the agent needs to send the requests: URLs, headers and bodies already
resolved with the environment (requests using dynamic values such as `{{$uuid}}` are sent as
templates plus their variables and re-resolved on the agent). If a request uses `{{secret.X}}`,
the agents would receive the **resolved secret values** — so the coordinator **refuses** unless you
explicitly allow it:

* CLI: `--allow-secrets-to-agents`
* App: tick **Send resolved secrets to agents** in the Distribute dialog (shown with the list of
  requests that use secrets).

Only the secrets the test's requests use are sent; agents mask them in everything they report, and
the coordinator masks them again before writing the summary. Prefer TLS (above) when you allow it.

## Limits

* Requests that need files on the coordinator — client certificates (mTLS), multipart file
  uploads, and binary file bodies combined with dynamic values — are refused for distributed runs.
* Mock baselines (`target: mock`, `--against-mock`) run against this machine's mock server and stay
  local.
* `{{$env.NAME}}` in a request with dynamic values is resolved on the agent (its own environment).
* Agents must run the same `devall` protocol version as the coordinator (checked up front).
* Agent clocks are not synchronised; starts are aligned by a fixed delay after each agent receives
  its plan (network latency apart, within milliseconds on a LAN).

# syntax=docker/dockerfile:1.7
#
# /dev/all images, both built from the `devall` CLI:
#
# * mock server (the default target): serves a project's mocks/ (WireMock-compatible) in CI or
#   on any host. Mount the project at /project:
#
#     docker build -t devall-mock .
#     docker run --rm -p 8080:8080 -v "$PWD:/project:ro" devall-mock
#     docker run --rm -p 8080:8080 -v "$PWD:/project:ro" devall-mock --chaos flaky --log json
#
#   Arguments after the image name are appended to `devall mock serve …` (later flags win).
#   Other commands: `docker run --rm -v "$PWD:/project" --entrypoint devall devall-mock mock check --project /project`.
#   See docs/MOCKS_IN_CI.md.
#
# * load agent (`--target agent`): a worker for distributed load tests
#   (`devall load run … --agents host:7777`). Give it the shared token:
#
#     docker build --target agent -t devall-agent .
#     docker run --rm -p 7777:7777 -e DEVALL_AGENT_TOKEN="$(devall load agent token)" devall-agent
#
#   TLS: mount a certificate and append `--tls-cert /certs/cert.pem --tls-key /certs/key.pem`,
#   or put a TLS terminator in front. Kubernetes: `devall load agent k8s --image …`.
#   See docs/LOAD_AGENTS.md.

ARG RUST_VERSION=1
ARG DEBIAN_RELEASE=bookworm

# ---------- build ----------
FROM rust:${RUST_VERSION}-${DEBIAN_RELEASE} AS build
# cmake/clang: aws-lc-sys (rustls crypto) and bundled SQLite/QuickJS build with the C toolchain.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang pkg-config \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The whole workspace is needed to resolve Cargo.lock (see .dockerignore for what is left out);
# only the CLI is compiled.
COPY . .
# Thin LTO instead of the release profile's fat LTO: a much faster image build, nearly the same binary.
ENV CARGO_PROFILE_RELEASE_LTO=thin
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p devall-cli \
 && cp target/release/devall /usr/local/bin/devall

# ---------- runtime base ----------
FROM debian:${DEBIAN_RELEASE}-slim AS runtime
# ca-certificates: proxy/record stubs and load tests talk to real HTTPS APIs.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 10001 devall
COPY --from=build /usr/local/bin/devall /usr/local/bin/devall
USER devall

# ---------- load agent ----------
FROM runtime AS agent
EXPOSE 7777
HEALTHCHECK --interval=10s --timeout=5s --start-period=5s --retries=3 CMD ["devall", "load", "agent", "health", "--port", "7777"]
ENTRYPOINT ["devall", "load", "agent", "--listen", "0.0.0.0:7777"]
CMD []

# ---------- mock server (default) ----------
FROM runtime AS mock
WORKDIR /project
# 8080 HTTP (+ GraphQL, WebSocket); 50051 gRPC and 1883 MQTT start when mocks/grpc or mocks/mqtt exist.
EXPOSE 8080 50051 1883
HEALTHCHECK --interval=10s --timeout=5s --start-period=5s --retries=3 CMD ["devall", "mock", "health", "--port", "8080"]
ENTRYPOINT ["devall", "mock", "serve", "--project", "/project", "--bind", "0.0.0.0", "--port", "8080"]
CMD []

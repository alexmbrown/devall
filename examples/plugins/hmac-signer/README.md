# hmac-signer — example /dev/all plugin

An auth provider that signs every request with HMAC-SHA256, and a utility op that computes an
HMAC of the input text. Written with the Rust SDK in `sdk/rust/devall-plugin`.

## Build

```sh
rustup target add wasm32-unknown-unknown   # once
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/hmac_signer.wasm plugin.wasm
```

Unit tests run on your machine (`cargo test`); host calls (time, randomness, secrets) only work
inside /dev/all.

## Install

- App: **Plugins → Install from folder…** and pick this folder (it needs `plugin.yaml` and
  `plugin.wasm`). Tick `HMAC_SECRET` if you want the plugin to read that secret, then set its value
  with **Set…**.
- CLI: `devall plugin install examples/plugins/hmac-signer --grant-secret HMAC_SECRET`
- Or commit it to a project as `plugins/hmac-signer/` — everyone who opens the project is asked to
  trust it first.

## Use

- Utilities → **Plugins → HMAC-SHA256 sign**, or `devall util hmac-signer:sign "hello" --arg key=k`.
- A request's **Auth → Plugin… → HMAC request signature**, which writes:

```yaml
auth:
  type: plugin
  plugin: hmac-signer
  provider: hmac
  config:
    keyId: client-1
    secret: "{{secret.HMAC_KEY}}"
```

The signed string is `METHOD\nPATH?QUERY\nUNIX_SECONDS\nNONCE\nHEX(SHA256(body))`; the
signature is Base64 and goes into
`Authorization: HMAC-SHA256 keyId="…", ts="…", nonce="…", signature="…"` with
`X-Content-SHA256` carrying the body hash.

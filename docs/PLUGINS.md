# Plugins

/dev/all plugins are small, sandboxed WebAssembly modules. A plugin can add:

| Contribution | Where it shows up | Call kind |
|---|---|---|
| **Utility ops** | Utilities page (category *Plugins*), command palette, `devall util <plugin>:<op>`, `devall plugin run` | `utility` |
| **Request hooks** | Every send and collection run (app and `devall run`; not load tests): pre-request transform, post-response tests / variables | `hook.preRequest`, `hook.postResponse` |
| **Import formats** | Import dialog (asked when no built-in format recognises the input) | `import.detect`, `import.parse` |
| **Codegen targets** | Generate code → *Plugins* group | `codegen` |
| **Auth providers** | A request's / folder's Auth → *Plugin…* (`auth: { type: plugin }`) | `auth` |

The runtime is `crates/devall-plugins` (wasmi interpreter); the Rust SDK is
`sdk/rust/devall-plugin`; a complete example is `examples/plugins/hmac-signer` (auth provider +
utility).

## A plugin on disk

```
my-plugin/
  plugin.yaml     manifest
  plugin.wasm     module built for wasm32-unknown-unknown
```

Installed plugins live in `<app data>/plugins/<id>/` (installed from a folder or a `.zip` whose
root, or single top-level folder, holds `plugin.yaml`). Project plugins live in
`<project>/plugins/<id>/` and are committed with the repository.

### `plugin.yaml`

```yaml
id: hmac-signer            # [a-z0-9][a-z0-9._-]{0,63}; the install folder name
name: HMAC signer
version: 0.1.0
apiVersion: 1              # the contract version below
description: Signs requests with HMAC-SHA256.
author: Example Inc.       # optional
homepage: https://…        # optional
wasm: plugin.wasm          # optional, default plugin.wasm
capabilities:              # everything is denied unless listed here AND approved by the user
  http:
    hosts: [api.example.com, "*.internal.example", "localhost:8080"]
  secrets.read: [HMAC_SECRET]     # plugin secrets, by name
  fs.project.read: true           # read files of the open project
contributes:
  utilities:
    - id: sign
      name: HMAC-SHA256 sign
      description: HMAC of the input text
      category: Crypto             # default "Plugins"
      output: text                 # or json (pretty-printed)
      noInput: false               # generators that ignore the input
      example: hello world         # the Utilities page's Example button
      args:
        - { name: key, label: Key, kind: secret }               # kinds: text, secret, number, textarea, select:a|b
        - { name: encoding, kind: "select:hex|base64", default: hex }
  hooks:
    - id: trace
      name: Add a trace header
      events: [preRequest, postResponse]
      hosts: [api.example.com]     # optional: only requests to these hosts
  importers:
    - { id: myfmt, name: My format, extensions: [.myfmt] }
  codegen:
    - { id: ktor, name: Kotlin (Ktor), language: kotlin }        # editor language for highlighting
  auth:
    - id: hmac
      name: HMAC request signature
      description: Shown under the provider picker.
      fields:                                                     # the auth editor's inputs
        - { name: keyId, label: Key ID }
        - { name: secret, label: Secret, kind: secret }
```

Unknown keys are rejected (a typo in a capability name must not silently drop a permission), ids
must be unique per kind, and a manifest must contribute at least one thing.

## Install, trust and permissions

- **Installed plugins.** *Plugins → Install from folder… / .zip…* or
  `devall plugin install <folder|zip> [--grant-secret NAME] [--yes]` shows the manifest, the
  module's SHA-256 and the capabilities as sentences ("Make HTTP requests to api.example.com",
  "Read its secrets HMAC_SECRET", "Read files in the open project"). The user picks which declared
  secrets the plugin may read. The install is pinned to the hash that was reviewed.
- **Project plugins never run until trusted** on each machine (*Plugins → Review and trust…* or
  `devall plugin trust <id>`). Trust is per project id and pinned to the module's SHA-256 **and**
  the capabilities as approved: a `git pull` that changes either marks the plugin *Changed* and it
  stops running until reviewed again. An installed plugin with the same id wins; the project copy
  is reported as *Shadowed*.
- Approvals (hashes, granted secrets, enabled flag, disabled hooks) are stored in
  `<app data>/plugins.json` — never in a project, so a repository cannot approve itself. A plugin
  copied into `<app data>/plugins/` by hand is untrusted until approved too.
- Plugins can be disabled, hooks toggled one by one, grants changed, trust revoked; uninstall moves
  the folder to the OS trash.
- **Plugin secrets** are values the user stores for the plugin (*Plugins → Set…*) in the OS keychain
  under the scope `plugin:<id>`; `secrets.get` only returns names the manifest declares **and** the
  user granted. Plugins never see project secrets except values that the request itself resolved
  (e.g. an auth config field written as `{{secret.HMAC_KEY}}`).

## Sandbox

- No WASI: no clock, files, network, environment or randomness. The only imports are
  `devall.log` and `devall.host_call`; a module importing anything else (e.g. a `wasm32-wasip1`
  build) is refused at install with the import's name.
- Every call runs in a **fresh instance** (nothing survives between calls).
- **Limits** per call: fuel (≈ instructions) 1 000 000 000 and 5 s wall clock for utilities,
  importers and codegen; 500 000 000 and 2 s for hooks and auth (they sit on the request path);
  64 MiB linear memory; 16 MiB output; 8 HTTP requests. Fuel is handed out in slices so the
  wall-clock deadline is checked between slices; growing memory past the cap fails `memory.grow`.
- Failures are deterministic and say what happened:
  `plugin x: exceeded its instruction budget (1000000000 fuel units)`,
  `plugin x: timed out after 2000 ms`, `plugin x: exceeded its memory limit (64 MiB)`,
  `plugin x: capability denied: http request to evil.example is not allowed (allowed hosts: api.example.com)`,
  `plugin x: crashed (…)`, or the plugin's own error message.
- A denied host call returns `{"error": "capability denied: …"}` to the plugin; if the call then
  fails, the failure is reported as that denial regardless of how the plugin worded its error.

## ABI (apiVersion 1)

All values are wasm `i32` / `i64`; pointers are offsets into the exported `memory`.

| Export | Signature | Meaning |
|---|---|---|
| `memory` | memory | linear memory |
| `alloc` | `(len: i32) -> i32` | memory for the host to write `len` bytes into; ownership passes to the reader |
| `dealloc` | `(ptr: i32, len: i32)` | optional |
| `devall_call` | `(ptr: i32, len: i32) -> i64` | reads a call envelope (UTF-8 JSON) at `ptr`, returns `(out_ptr << 32) \| out_len` of `{"ok": …}` or `{"error": "…"}` |

| Import (`devall` module) | Signature | Meaning |
|---|---|---|
| `log` | `(level: i32, ptr: i32, len: i32)` | 0 debug, 1 info, 2 warn, 3 error; shown with hook / utility results (≤ 200 lines, 64 KiB) |
| `host_call` | `(ptr: i32, len: i32) -> i64` | JSON request in; the reply (`{"ok": …}` / `{"error": "…"}`) is written into memory the host gets from the plugin's own `alloc` and returned packed like `devall_call` |

Call envelope:

```json
{ "apiVersion": 1, "kind": "utility", "id": "sign", "input": { … } }
```

## Contracts

Shared shapes:

```jsonc
// Request (resolved: variables and secrets substituted)
{ "method": "POST", "url": "https://api.example.com/v1/items?x=1",
  "headers": [{ "name": "Content-Type", "value": "application/json" }],
  "body": "{\"a\":1}",            // absent = no body; multipart bodies are not passed
  "bodyBase64": false }           // true when body is Base64 (binary)
// Response
{ "status": 200, "headers": [{ "name": "…", "value": "…" }], "body": "…", "bodyBase64": false, "timeMs": 42 }
```

| Kind | `input` | `ok` value |
|---|---|---|
| `utility` | `{ "input": "text", "args": { "name": "value" } }` (missing args take the manifest defaults) | `{ "output": "text" }` or `{ "output": <any JSON> }` |
| `hook.preRequest` | `{ "request": Request, "requestName"?: "…", "environment"?: "…" }` | `{ "request"?: Request, "logs"?: ["…"] }` — the request to send instead (omit = unchanged) |
| `hook.postResponse` | `{ "request": Request, "response": Response, "requestName"?: "…" }` | `{ "tests"?: [{ "name", "passed", "message"? }], "variables"?: { "k": "v" }, "logs"?: ["…"] }` |
| `import.detect` | `{ "text": "…", "fileName"?: "x.myfmt" }` | `{ "confidence": 0.0–1.0 }` (≥ 0.5 counts; a matching file extension counts as 0.5) |
| `import.parse` | same as detect | `{ "name"?: "…", "requests": [ImportedRequest], "warnings"?: ["…"] }` |
| `codegen` | `{ "request": Request }` (secret values masked unless the user ticks *Include secrets*) | `{ "code": "…" }` |
| `auth` | `{ "request": Request, "config": { "field": "resolved value" } }` | `{ "headers": [Header], "query"?: [Header] }` — headers replace same-named ones, query parameters are appended |

`ImportedRequest`:

```json
{ "name": "List items", "method": "GET", "url": "https://api.example.com/items",
  "headers": [{ "name": "Accept", "value": "application/json" }],
  "body": { "type": "json", "content": "{}" },        // or {"type":"text","content","contentType"?} / {"type":"form","fields":[Header]}
  "folder": "items/v1", "docs": "Markdown notes" }
```

Hooks run in order (installed plugins first, by id); each pre-request hook sees the previous one's
output. A failing pre-request hook fails the send (like a failing pre-request script); a failing
post-response hook adds a failed test row. Hook logs appear in the response's script console.

### Host calls

| `op` | Request | `ok` reply | Needs |
|---|---|---|---|
| `http.request` | `{ "op": "http.request", "request": Request, "timeoutMs"?: 5000 }` | Response | `http` with the URL's host (and port, if the pattern has one) allowed; redirects are not followed |
| `secrets.get` | `{ "op": "secrets.get", "name": "HMAC_SECRET" }` | `{ "value": "…" }` | `secrets.read` listing the name, granted by the user |
| `fs.read` | `{ "op": "fs.read", "path": "openapi/api.yaml" }` | `{ "content": "…", "base64": false }` | `fs.project.read`; inside the project, never `.git/` or `.devall/`, ≤ 4 MiB |
| `time.now` | `{ "op": "time.now" }` | `{ "unixMs": …, "iso": "2026-…Z" }` | — |
| `random.bytes` | `{ "op": "random.bytes", "len": 16 }` | `{ "hex": "…" }` (≤ 1024 bytes, OS RNG) | — |

## Writing a plugin in Rust

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
devall-plugin = { path = "…/sdk/rust/devall-plugin" }
```

```rust
use devall_plugin::*;

struct Upper;

impl Plugin for Upper {
    fn utility(&self, id: &str, input: UtilityInput) -> Result<UtilityOutput, String> {
        match id {
            "upper" => Ok(UtilityOutput::text(input.input.to_uppercase())),
            other => Err(format!("unknown utility {other}")),
        }
    }

    fn auth(&self, _id: &str, input: AuthInput) -> Result<AuthOutput, String> {
        let (ms, _) = host::now()?;
        Ok(AuthOutput { headers: vec![Header::new("X-Signed-At", ms.to_string())], query: vec![] })
    }
}

devall_plugin::export!(Upper);
```

```sh
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/<crate>.wasm plugin.wasm
devall plugin install .                      # or Plugins → Install from folder…
devall util my-plugin:upper "hello"          # → HELLO
```

Only implement the trait methods for what the manifest declares; the others answer with an
error. `devall_plugin::dispatch(&plugin, envelope_bytes)` runs a call without the ABI, so plugin
logic is unit-testable on the host (`cargo test`); host calls return an error outside /dev/all.
Release builds should use `opt-level = "s"`, `lto = true`, `panic = "abort"` (the example's
`Cargo.toml`) — modules are usually 100–300 KiB.

Any language that compiles to `wasm32-unknown-unknown` without WASI imports works (Zig, C via
clang, AssemblyScript, TinyGo with `-target=wasm-unknown`): export the three functions above and
speak JSON.

## CLI

```sh
devall plugin list [--json]                         # installed + project plugins, status, contributions
devall plugin install <folder|zip> [--grant-secret NAME] [--yes]
devall plugin trust <id> [--grant-secret NAME] [--yes]   # a project plugin, pinned to its current hash
devall plugin run <plugin>:<op> [input|-] [--arg k=v]
devall plugin uninstall <id>                        # to the OS trash
devall util list                                    # includes plugin ops
devall util <plugin>:<op> "input" --arg k=v
```

`devall run`, `devall load run` and `devall mcp` load the installed and trusted plugins, so
`auth: { type: plugin }` works there too; `devall run` (like the app's sends and collection runs)
also runs the request hooks. Load tests do not run hooks (per-request wasm calls would distort the
measurements).

## Not (yet) supported

- The component model / WIT: the ABI is plain exports + JSON.
- Async host calls and streaming; hooks see buffered bodies (multipart bodies are not passed).
- UI contributions (panels, editors) and settings pages.
- Plugins in hosted features (the relay never runs plugin code).

# Releasing /dev/all

How to build signed desktop bundles. The certificates and accounts are the owner's; nothing here
is automated in CI yet, and no secret ever goes into the repository.

## 1. The bundled llama.cpp sidecar (`externalBin`)

The managed local model runs `llama-server` from the pinned llama.cpp build
(`crates/devall-ai/src/catalog.rs`, `LLAMA_CPP_BUILD`). Release bundles ship it so users don't
have to download it; development builds don't need it.

```sh
cd apps/desktop
npm run app:bundle
# = node ../../scripts/fetch-llama-server.mjs
#   && tauri build --config src-tauri/tauri.bundle.conf.json --config src-tauri/binaries/tauri.libs.conf.json
```

* `scripts/fetch-llama-server.mjs` (Node 18+, no dependencies) downloads the release asset for the
  current Rust target triple (or `--target <triple>` / `--variant <asset>`; `--dry-run` prints the
  plan), verifies size and SHA-256 against `crates/devall-ai/llama-assets.json`, and extracts
  `llama-server` to `apps/desktop/src-tauri/binaries/llama-server-<target-triple>[.exe]` plus its
  shared libraries (Windows `ggml*.dll`/`llama*.dll`; macOS `*.dylib`; Linux `*.so`) into the same
  folder. Downloads are cached in `target/llama-cache/`. `binaries/` is git-ignored.
* `llama-assets.json` is generated from `catalog.rs`; a test fails when they differ. After bumping
  the build or a hash: `UPDATE_LLAMA_ASSETS=1 cargo test -p devall-ai llama_assets_json`.
* `src-tauri/tauri.bundle.conf.json` adds `bundle.externalBin: ["binaries/llama-server"]`.
  It is an overlay, not part of `tauri.conf.json`, because `tauri-build` copies external
  binaries and resources on **every** build — `cargo check` and `tauri dev` would fail without
  the file. The script writes `binaries/tauri.libs.conf.json` with the `bundle.resources` for the
  libraries: on Windows they land next to `devall-desktop.exe` (where `llama-server.exe` looks
  first); on macOS in `Contents/Resources/llama/` (the script adds the
  `@executable_path/../Resources/llama` rpath with `install_name_tool`); on Linux in
  `…/llama/` (`patchelf` sets `$ORIGIN`-relative rpaths; best-effort platform).
* Defaults: CPU builds on Windows/Linux (no GPU runtime needed — users can install the Vulkan or
  CUDA build from AI → Models), Metal on Apple Silicon. `--variant win-vulkan-x64` bundles Vulkan
  instead (needs `vulkan-1.dll` from the GPU driver on the user's machine).
* At runtime the app resolves the server in this order: `DEVALL_LLAMA_SERVER` → a configured path
  → the bundled binary next to the executable → a build installed from AI → Models.

## 2. Windows: Authenticode

Sign **every** executable and DLL — the app, `llama-server.exe` and the llama.cpp DLLs. Unsigned
llama.cpp binaries have repeatedly been quarantined by Windows Defender as false positives.

Tauri signs the app, the sidecar and the installer when `bundle.windows` has signing settings, so
put them in a local, untracked overlay (or pass them with `--config` in CI from secrets):

* **Certificate in the Windows store / hardware token (EV/OV):**
  ```json
  { "bundle": { "windows": {
      "certificateThumbprint": "<SHA-1 thumbprint>",
      "digestAlgorithm": "sha256",
      "timestampUrl": "http://timestamp.digicert.com"
  } } }
  ```
* **Azure Trusted Signing** (no local key; the owner's Azure account): install the
  `trusted-signing-cli` (`cargo install trusted-signing-cli`), log in (`az login` or the
  `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET` / `AZURE_TENANT_ID` environment variables) and use a
  custom sign command:
  ```json
  { "bundle": { "windows": {
      "signCommand": "trusted-signing-cli -e https://<region>.codesigning.azure.net -a <account> -c <certificate-profile> %1"
  } } }
  ```

The DLLs added as resources are not signed by Tauri's bundler — sign them before bundling:

```powershell
signtool sign /fd sha256 /tr http://timestamp.digicert.com /td sha256 /sha1 <thumbprint> `
  apps\desktop\src-tauri\binaries\*.dll apps\desktop\src-tauri\binaries\llama-server-*.exe
# or: trusted-signing-cli -e … -a … -c … <file> (once per file)
signtool verify /pa /v apps\desktop\src-tauri\target\release\bundle\nsis\*.exe
```

## 3. macOS: codesign + notarization

Needs a **Developer ID Application** certificate in the login keychain and an App Store Connect
API key (or Apple ID + app-specific password) for `notarytool`.

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: <Name> (<TEAMID>)"
# notarization (one of):
export APPLE_API_ISSUER=<issuer-id> APPLE_API_KEY=<key-id> APPLE_API_KEY_PATH=~/keys/AuthKey_<key-id>.p8
# or: export APPLE_ID=<apple-id> APPLE_PASSWORD=<app-specific-password> APPLE_TEAM_ID=<TEAMID>
npm run app:bundle
```

Tauri signs the app and the `externalBin` with the hardened runtime and notarizes the bundle
when these variables are set. The dylibs in `Contents/Resources/llama/` must be signed too, and
the sidecar needs entitlements for the hardened runtime (Metal and loading its own libraries):

```xml
<!-- apps/desktop/src-tauri/sidecar.entitlements (owner-provided; reference it from the macOS
     overlay as bundle.macOS.entitlements, or sign the sidecar separately with it) -->
<plist version="1.0"><dict>
  <key>com.apple.security.cs.disable-library-validation</key><true/>
  <key>com.apple.security.cs.allow-jit</key><true/>
</dict></plist>
```

```sh
# Sign libraries first, then the sidecar (inside-out), before `tauri build`:
codesign --force --timestamp --options runtime --sign "$APPLE_SIGNING_IDENTITY" apps/desktop/src-tauri/binaries/*.dylib
codesign --force --timestamp --options runtime --entitlements apps/desktop/src-tauri/sidecar.entitlements \
  --sign "$APPLE_SIGNING_IDENTITY" apps/desktop/src-tauri/binaries/llama-server-*-apple-darwin
# After bundling, check:
codesign --verify --deep --strict --verbose=2 "apps/desktop/src-tauri/target/release/bundle/macos/devall.app"
spctl --assess --type execute --verbose "apps/desktop/src-tauri/target/release/bundle/macos/devall.app"
xcrun stapler validate "apps/desktop/src-tauri/target/release/bundle/dmg/"*.dmg
```

(`install_name_tool` invalidates signatures, so the fetch script runs before signing.)

## 4. Checklist

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`
- [ ] `cd apps/desktop && npx tsc --noEmit -p tsconfig.json && npx vitest run`
- [ ] Version bumped in `Cargo.toml` (workspace) and `apps/desktop/src-tauri/tauri.conf.json`
- [ ] `npm run app:bundle` on Windows (x64) and macOS (arm64; x64 if shipped)
- [ ] Signatures verified (`signtool verify`, `codesign --verify`, `spctl --assess`), notarization stapled
- [ ] Smoke test: AI → Models shows the bundled server; start a small model; MCP endpoint starts

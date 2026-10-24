#!/usr/bin/env node
// Fetches the pinned llama.cpp `llama-server` for release bundling (Tauri `externalBin`).
//
//   node scripts/fetch-llama-server.mjs                       # this machine's target triple
//   node scripts/fetch-llama-server.mjs --target aarch64-apple-darwin
//   node scripts/fetch-llama-server.mjs --variant win-vulkan-x64
//   node scripts/fetch-llama-server.mjs --dry-run             # print the plan, download nothing
//
// What it does (no npm dependencies, Node 18+):
//   1. Picks the pinned release asset for the target from crates/devall-ai/llama-assets.json
//      (generated from crates/devall-ai/src/catalog.rs; a Rust test keeps them in sync).
//   2. Downloads it into target/llama-cache/ (skipped when a verified copy is there) and verifies
//      the size and SHA-256 — a mismatch aborts and deletes the file.
//   3. Extracts it with the system `tar` (bsdtar on Windows 10+/macOS reads .zip and .tar.gz) and
//      copies `llama-server` to apps/desktop/src-tauri/binaries/llama-server-<triple>[.exe] plus the
//      shared libraries next to it (ggml*/llama*.dll on Windows, *.dylib on macOS, *.so on Linux).
//   4. macOS: adds the rpath `@executable_path/../Resources/llama` (install_name_tool) so the
//      bundled binary finds its dylibs; Linux: sets `$ORIGIN`-relative rpaths with patchelf.
//   5. Writes apps/desktop/src-tauri/binaries/tauri.libs.conf.json: the `bundle.resources` entries
//      for those libraries (Windows: next to the app executable; macOS/Linux: `llama/`).
//
// Then bundle with both overlays (they are not part of tauri.conf.json, so `tauri dev` and
// `cargo check` work without the binary):
//   cd apps/desktop && npx tauri build --config src-tauri/tauri.bundle.conf.json --config src-tauri/binaries/tauri.libs.conf.json
// Sign the sidecar and its libraries with the app (see docs/RELEASING.md).

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const ASSETS = path.join(REPO, 'crates', 'devall-ai', 'llama-assets.json');
const BIN_DIR = path.join(REPO, 'apps', 'desktop', 'src-tauri', 'binaries');
const CACHE = path.join(REPO, 'target', 'llama-cache');

/** Default llama.cpp build per Rust target triple: CPU builds on Windows/Linux (no GPU runtime
 * needed — users can install the Vulkan/CUDA build in AI → Models), Metal on Apple Silicon. */
const DEFAULT_VARIANT = {
  'x86_64-pc-windows-msvc': 'win-cpu-x64',
  'aarch64-pc-windows-msvc': 'win-cpu-arm64',
  'aarch64-apple-darwin': 'macos-arm64',
  'x86_64-apple-darwin': 'macos-x64',
  'x86_64-unknown-linux-gnu': 'ubuntu-x64',
  'aarch64-unknown-linux-gnu': 'ubuntu-arm64',
};

function args() {
  const out = { target: undefined, variant: undefined, dryRun: false, force: false };
  const a = process.argv.slice(2);
  for (let i = 0; i < a.length; i++) {
    const [k, inline] = a[i].split('=');
    const val = () => inline ?? a[++i];
    if (k === '--target') out.target = val();
    else if (k === '--variant') out.variant = val();
    else if (k === '--dry-run') out.dryRun = true;
    else if (k === '--force') out.force = true;
    else if (k === '--help' || k === '-h') {
      console.log(fs.readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n').filter((l) => l.startsWith('//')).map((l) => l.slice(3)).join('\n'));
      process.exit(0);
    } else throw new Error(`unknown argument ${a[i]}`);
  }
  return out;
}

function hostTriple() {
  try {
    const out = execFileSync('rustc', ['-vV'], { encoding: 'utf8' });
    const m = /^host:\s*(\S+)/m.exec(out);
    if (m) return m[1];
  } catch {
    /* fall through */
  }
  const arch = process.arch === 'arm64' ? 'aarch64' : 'x86_64';
  if (process.platform === 'win32') return `${arch}-pc-windows-msvc`;
  if (process.platform === 'darwin') return `${arch}-apple-darwin`;
  return `${arch}-unknown-linux-gnu`;
}

const osOf = (triple) => (triple.includes('windows') ? 'windows' : triple.includes('apple') ? 'macos' : 'linux');

async function sha256(file) {
  const h = createHash('sha256');
  await pipeline(fs.createReadStream(file), h);
  return h.digest('hex');
}

async function download(spec) {
  fs.mkdirSync(CACHE, { recursive: true });
  const dest = path.join(CACHE, spec.file);
  if (fs.existsSync(dest) && fs.statSync(dest).size === spec.sizeBytes && (await sha256(dest)) === spec.sha256) {
    console.log(`✓ cached ${spec.file}`);
    return dest;
  }
  console.log(`↓ ${spec.url} (${(spec.sizeBytes / 1024 / 1024).toFixed(1)} MB)`);
  const res = await fetch(spec.url, { redirect: 'follow' });
  if (!res.ok || !res.body) throw new Error(`download failed: HTTP ${res.status} for ${spec.url}`);
  const part = `${dest}.part`;
  const hash = createHash('sha256');
  let size = 0;
  const body = Readable.fromWeb(res.body);
  body.on('data', (c) => {
    hash.update(c);
    size += c.length;
  });
  await pipeline(body, fs.createWriteStream(part));
  const got = hash.digest('hex');
  if (size !== spec.sizeBytes || got !== spec.sha256) {
    fs.rmSync(part, { force: true });
    throw new Error(`checksum mismatch for ${spec.file}: expected ${spec.sha256} (${spec.sizeBytes} B), got ${got} (${size} B) — not using it`);
  }
  fs.renameSync(part, dest);
  console.log(`✓ verified SHA-256 ${got}`);
  return dest;
}

function extract(archive, dir) {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  execFileSync('tar', ['-xf', archive, '-C', dir], { stdio: 'inherit' });
}

function findFile(dir, name, depth = 5) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isFile() && e.name === name) return p;
    if (e.isDirectory() && depth > 0) {
      const f = findFile(p, name, depth - 1);
      if (f) return f;
    }
  }
  return null;
}

const isLib = (os, name) => (os === 'windows' ? /\.dll$/i.test(name) : os === 'macos' ? /\.dylib$/.test(name) : /\.so(\.\d+)*$/.test(name));

/** Plan for a target: which asset, where things go, how the overlay maps libraries. */
export function plan(assets, triple, variant) {
  const v = variant ?? DEFAULT_VARIANT[triple];
  if (!v) throw new Error(`no default llama.cpp build for ${triple}; pass --variant (one of ${assets.assets.map((a) => a.variant).join(', ')})`);
  const asset = assets.assets.find((a) => a.variant === v);
  if (!asset) throw new Error(`variant ${v} is not in llama-assets.json (build ${assets.build})`);
  const os = osOf(triple);
  const exe = os === 'windows' ? 'llama-server.exe' : 'llama-server';
  return { triple, variant: v, os, asset, exe, sidecar: `llama-server-${triple}${os === 'windows' ? '.exe' : ''}` };
}

/** `bundle.resources` for the copied libraries (paths relative to src-tauri/). */
export function libsOverlay(os, libs) {
  const resources = {};
  for (const l of libs) resources[`binaries/${l}`] = os === 'windows' ? l : `llama/${l}`;
  return { bundle: Object.keys(resources).length ? { resources } : {} };
}

async function main() {
  const opts = args();
  const assets = JSON.parse(fs.readFileSync(ASSETS, 'utf8'));
  const p = plan(assets, opts.target ?? hostTriple(), opts.variant);
  console.log(`llama.cpp ${assets.build} · ${p.variant} → binaries/${p.sidecar}`);
  if (opts.dryRun) {
    console.log(JSON.stringify({ download: [p.asset.url, p.asset.runtime?.url].filter(Boolean), sha256: p.asset.sha256, sidecar: path.join(BIN_DIR, p.sidecar) }, null, 2));
    return;
  }
  const dest = path.join(BIN_DIR, p.sidecar);
  if (fs.existsSync(dest) && !opts.force) {
    console.log(`binaries/${p.sidecar} exists — use --force to replace it`);
  }
  const work = path.join(CACHE, `extract-${p.variant}`);
  extract(await download(p.asset), work);
  if (p.asset.runtime) {
    // CUDA builds: the runtime DLLs go next to the server too.
    extract(await download(p.asset.runtime), `${work}-runtime`);
  }
  const server = findFile(work, p.exe);
  if (!server) throw new Error(`${p.exe} not found in ${p.asset.file}`);
  fs.mkdirSync(BIN_DIR, { recursive: true });
  fs.copyFileSync(server, dest);
  if (p.os !== 'windows') fs.chmodSync(dest, 0o755);

  const libDirs = [path.dirname(server), ...(p.asset.runtime ? [`${work}-runtime`] : [])];
  const libs = [];
  for (const d of libDirs) {
    for (const name of fs.readdirSync(d)) {
      if (!isLib(p.os, name)) continue;
      fs.copyFileSync(path.join(d, name), path.join(BIN_DIR, name));
      libs.push(name);
    }
  }
  if (p.os === 'macos' && libs.length) {
    try {
      execFileSync('install_name_tool', ['-add_rpath', '@executable_path/../Resources/llama', dest], { stdio: 'inherit' });
    } catch {
      console.warn('! install_name_tool failed (rpath may already be set) — check with `otool -l` before signing');
    }
  }
  if (p.os === 'linux' && libs.length) {
    try {
      execFileSync('patchelf', ['--set-rpath', '$ORIGIN:$ORIGIN/../lib/devall/llama', dest], { stdio: 'inherit' });
    } catch {
      console.warn('! patchelf not found: the Linux bundle needs the libraries on the loader path (best-effort platform)');
    }
  }
  const overlay = path.join(BIN_DIR, 'tauri.libs.conf.json');
  fs.writeFileSync(overlay, `${JSON.stringify(libsOverlay(p.os, libs), null, 2)}\n`);
  console.log(`✓ binaries/${p.sidecar}${libs.length ? ` + ${libs.length} libraries (${libs.join(', ')})` : ''}`);
  console.log('Next (from apps/desktop):');
  console.log('  npx tauri build --config src-tauri/tauri.bundle.conf.json --config src-tauri/binaries/tauri.libs.conf.json');
  console.log('Sign the sidecar and libraries with the app — docs/RELEASING.md.');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error(`error: ${e.message}`);
    process.exit(1);
  });
}

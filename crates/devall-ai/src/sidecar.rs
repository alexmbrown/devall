//! Managed `llama-server` sidecar (opt-in).
//!
//! Binary resolution order: `DEVALL_LLAMA_SERVER` env var → a path the user configured → a
//! binary bundled with the app (Tauri `externalBin`) → a build downloaded into
//! `<data>/ai/llama.cpp/<build>-<variant>/` from the pinned llama.cpp release (SHA-256 verified).
//!
//! The server always binds `127.0.0.1` on a random free port and requires a random API key, so
//! other local processes and DNS-rebinding web pages can't use it. Its web UI is disabled.
//! Windows antivirus products have repeatedly flagged llama.cpp binaries as false positives; a
//! binary that vanishes or fails to start produces an explicit "quarantined?" hint.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::catalog::{self, LLAMA_CPP_BUILD, LlamaAsset};
use crate::download::{self, CancelFlag, DownloadSpec, Progress};
use crate::hardware::LlamaVariant;
use crate::{AiError, Result};

pub const SERVER_ENV: &str = "DEVALL_LLAMA_SERVER";

pub fn server_exe_name() -> &'static str {
    if cfg!(windows) { "llama-server.exe" } else { "llama-server" }
}

/// `<data>/ai`
pub fn ai_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("ai")
}

pub fn models_dir(data_dir: &Path) -> PathBuf {
    ai_dir(data_dir).join("models")
}

/// Where a catalog model is stored: `<data>/ai/models/<repo with __>/<file>`.
pub fn model_path(data_dir: &Path, m: &catalog::CatalogModel) -> PathBuf {
    models_dir(data_dir).join(m.hf_repo.replace('/', "__")).join(m.hf_file)
}

/// Download state of a catalog model.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFileStatus {
    pub id: String,
    pub downloaded: bool,
    pub path: PathBuf,
    /// Bytes already in `<file>.part` (a resumable download).
    pub partial_bytes: u64,
}

pub fn model_status(data_dir: &Path, m: &catalog::CatalogModel) -> ModelFileStatus {
    let path = model_path(data_dir, m);
    let downloaded = std::fs::metadata(&path).is_ok_and(|md| md.len() == m.size_bytes);
    let mut part = path.as_os_str().to_os_string();
    part.push(".part");
    let partial_bytes = std::fs::metadata(PathBuf::from(part)).map(|md| md.len()).unwrap_or(0);
    ModelFileStatus { id: m.id.to_string(), downloaded, path, partial_bytes }
}

/// Moves a downloaded model (and any partial download) to the OS trash.
pub fn delete_model(data_dir: &Path, m: &catalog::CatalogModel) -> Result<()> {
    let st = model_status(data_dir, m);
    let mut part = st.path.as_os_str().to_os_string();
    part.push(".part");
    for p in [st.path.clone(), PathBuf::from(part)] {
        if p.exists() {
            trash::delete(&p).map_err(|e| AiError::Io { path: p.clone(), source: std::io::Error::other(e.to_string()) })?;
        }
    }
    Ok(())
}

/// Downloads a catalog model (resumable, SHA-256 verified) into [`model_path`].
pub async fn download_model(data_dir: &Path, m: &catalog::CatalogModel, cancel: &CancelFlag, on_progress: impl FnMut(&Progress)) -> Result<PathBuf> {
    let spec = DownloadSpec { url: m.download_url(), dest: model_path(data_dir, m), expected_size: Some(m.size_bytes), sha256: Some(m.sha256.to_string()) };
    download::download(&spec, cancel, on_progress).await
}

pub fn install_dir(data_dir: &Path, variant: LlamaVariant) -> PathBuf {
    ai_dir(data_dir).join("llama.cpp").join(format!("{LLAMA_CPP_BUILD}-{}", variant.asset_suffix()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub build: String,
    pub variant: LlamaVariant,
    pub server: PathBuf,
}

const MARKER: &str = "devall-install.json";

/// Builds already downloaded, newest-preferred order is the caller's choice.
pub fn installed(data_dir: &Path) -> Vec<Installed> {
    let root = ai_dir(data_dir).join("llama.cpp");
    let Ok(rd) = std::fs::read_dir(&root) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let bytes = std::fs::read(e.path().join(MARKER)).ok()?;
            let i: Installed = serde_json::from_slice(&bytes).ok()?;
            i.server.is_file().then_some(i)
        })
        .collect()
}

fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut dirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            dirs.push(p);
        } else if e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    dirs.into_iter().find_map(|d| find_file(&d, name, depth - 1))
}

/// Extracts a `.zip` or `.tar.gz` into `dest`, refusing entries that escape it.
pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let io = |e: std::io::Error| AiError::Io { path: archive.to_path_buf(), source: e };
    std::fs::create_dir_all(dest).map_err(io)?;
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let file = std::fs::File::open(archive).map_err(io)?;
    if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(|e| AiError::Sidecar(format!("bad archive {name}: {e}")))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| AiError::Sidecar(format!("bad archive {name}: {e}")))?;
            let Some(rel) = entry.enclosed_name() else { continue };
            let out = dest.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(io)?;
                continue;
            }
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(io)?;
            }
            let mut f = std::fs::File::create(&out).map_err(io)?;
            std::io::copy(&mut entry, &mut f).map_err(io)?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777));
            }
        }
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        // `unpack` skips entries with `..` or absolute paths.
        tar.unpack(dest).map_err(io)?;
    } else {
        return Err(AiError::Sidecar(format!("unsupported archive type: {name}")));
    }
    Ok(())
}

/// Downloads (resumable, SHA-256 pinned) and extracts the llama.cpp build for `variant`.
pub async fn install(data_dir: &Path, variant: LlamaVariant, cancel: &CancelFlag, mut on_progress: impl FnMut(&Progress)) -> Result<Installed> {
    let asset: LlamaAsset = catalog::llama_asset(variant).ok_or_else(|| AiError::Sidecar(format!("no prebuilt llama.cpp for {variant:?}")))?;
    let downloads = ai_dir(data_dir).join("downloads");
    let mut files = vec![(asset.file, asset.size_bytes, asset.sha256)];
    if let Some(rt) = asset.runtime {
        files.push(rt);
    }
    let total: u64 = files.iter().map(|f| f.1).sum();
    let mut done_bytes = 0u64;
    let mut archives = Vec::new();
    for (file, size, sha) in files {
        let spec = DownloadSpec { url: LlamaAsset::url(file), dest: downloads.join(file), expected_size: Some(size), sha256: Some(sha.to_string()) };
        let base = done_bytes;
        let path = download::download(&spec, cancel, |p| {
            on_progress(&Progress { downloaded: base + p.downloaded, total: Some(total), bytes_per_sec: p.bytes_per_sec, phase: if p.phase == "done" { "downloading" } else { p.phase }, resumed_from: p.resumed_from })
        })
        .await?;
        done_bytes += size;
        archives.push(path);
    }
    on_progress(&Progress { downloaded: total, total: Some(total), bytes_per_sec: 0.0, phase: "installing", resumed_from: 0 });
    let dir = install_dir(data_dir, variant);
    let dir2 = dir.clone();
    let archives2 = archives.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let _ = std::fs::remove_dir_all(&dir2);
        for a in &archives2 {
            extract(a, &dir2)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| AiError::Sidecar(e.to_string()))??;
    let server = find_file(&dir, server_exe_name(), 4).ok_or_else(|| {
        AiError::Sidecar(format!(
            "{} not found after extracting {} — if an antivirus removed it, restore it or allow the folder {}",
            server_exe_name(),
            asset.file,
            dir.display()
        ))
    })?;
    let inst = Installed { build: LLAMA_CPP_BUILD.into(), variant, server };
    let json = serde_json::to_vec_pretty(&inst).map_err(|e| AiError::Sidecar(e.to_string()))?;
    devall_core::fsutil::atomic_write(&dir.join(MARKER), &json)?;
    for a in archives {
        let _ = std::fs::remove_file(a);
    }
    on_progress(&Progress { downloaded: total, total: Some(total), bytes_per_sec: 0.0, phase: "done", resumed_from: 0 });
    Ok(inst)
}

/// Resolves the server binary (see module docs).
pub fn resolve_server(data_dir: &Path, configured: Option<&Path>, bundled: Option<&Path>, preferred: Option<LlamaVariant>) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(SERVER_ENV)
        && Path::new(&p).is_file()
    {
        return Some(PathBuf::from(p));
    }
    for p in [configured, bundled].into_iter().flatten() {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    let mut list = installed(data_dir);
    if let Some(v) = preferred {
        list.sort_by_key(|i| i.variant != v);
    }
    list.into_iter().map(|i| i.server).next()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SidecarOptions {
    pub context: u32,
    /// Layers to offload to the GPU (`None` = all when the build has a GPU backend).
    pub gpu_layers: Option<u32>,
    pub threads: Option<u32>,
    /// Name served at `/v1/models` (defaults to the model file stem).
    pub alias: Option<String>,
    pub gpu: bool,
    /// Serve `/v1/embeddings` with an embedding model (`--embedding`; the batch is sized to the
    /// context so a whole chunk fits in one micro-batch, as non-causal models require).
    pub embedding: bool,
}

impl Default for SidecarOptions {
    fn default() -> Self {
        Self { context: 16_384, gpu_layers: None, threads: None, alias: None, gpu: true, embedding: false }
    }
}

/// Command-line arguments for `llama-server`.
pub fn server_args(model: &Path, port: u16, api_key: &str, opts: &SidecarOptions) -> Vec<String> {
    let alias = opts.alias.clone().unwrap_or_else(|| model.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "local".into()));
    let mut a: Vec<String> = vec![
        "-m".into(),
        model.to_string_lossy().into_owned(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--api-key".into(),
        api_key.into(),
        "-c".into(),
        opts.context.to_string(),
        "--alias".into(),
        alias,
        "--jinja".into(),
        "--no-webui".into(),
    ];
    if opts.gpu {
        a.push("-ngl".into());
        a.push(opts.gpu_layers.unwrap_or(999).to_string());
    }
    if let Some(t) = opts.threads {
        a.push("-t".into());
        a.push(t.to_string());
    }
    if opts.embedding {
        for x in ["--embedding", "-b", &opts.context.to_string(), "-ub", &opts.context.to_string()] {
            a.push(x.to_string());
        }
    }
    a
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| AiError::Sidecar(format!("no free port: {e}")))?;
    Ok(l.local_addr().map_err(|e| AiError::Sidecar(e.to_string()))?.port())
}

fn random_key() -> String {
    let mut s = String::from("devall-");
    for _ in 0..4 {
        s.push_str(&format!("{:016x}", fastrand::u64(..)));
    }
    s
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarInfo {
    pub pid: Option<u32>,
    pub port: u16,
    pub base_url: String,
    pub model: String,
    pub model_path: PathBuf,
    pub server: PathBuf,
    pub started_at: i64,
    pub log_file: PathBuf,
}

/// A running `llama-server`. Killed on drop.
pub struct Sidecar {
    child: tokio::process::Child,
    pub info: SidecarInfo,
    api_key: String,
}

impl Sidecar {
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Spawns the server and waits until `/health` says it's ready (model loaded).
    pub async fn start(server: &Path, model: &Path, opts: &SidecarOptions, log_dir: &Path, ready_timeout: Duration) -> Result<Self> {
        if !server.is_file() {
            return Err(AiError::Sidecar(format!("{} is missing — reinstall llama.cpp (antivirus software sometimes quarantines it)", server.display())));
        }
        if !model.is_file() {
            return Err(AiError::Sidecar(format!("model file {} not found — download it first", model.display())));
        }
        std::fs::create_dir_all(log_dir).map_err(|e| AiError::Io { path: log_dir.to_path_buf(), source: e })?;
        let log_file = log_dir.join("llama-server.log");
        let log = std::fs::File::create(&log_file).map_err(|e| AiError::Io { path: log_file.clone(), source: e })?;
        let log_err = log.try_clone().map_err(|e| AiError::Io { path: log_file.clone(), source: e })?;
        let port = free_port()?;
        let api_key = random_key();
        let mut cmd = tokio::process::Command::new(server);
        cmd.args(server_args(model, port, &api_key, opts))
            .current_dir(server.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let child = cmd.spawn().map_err(|e| {
            AiError::Sidecar(format!("could not start {}: {e}. If your antivirus quarantined it, allow the folder or connect to Ollama/LM Studio instead.", server.display()))
        })?;
        let base_url = format!("http://127.0.0.1:{port}");
        let alias = opts.alias.clone().unwrap_or_else(|| model.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
        let mut me = Sidecar {
            info: SidecarInfo {
                pid: child.id(),
                port,
                base_url: base_url.clone(),
                model: alias,
                model_path: model.to_path_buf(),
                server: server.to_path_buf(),
                started_at: chrono_now_ms(),
                log_file: log_file.clone(),
            },
            child,
            api_key,
        };
        let deadline = Instant::now() + ready_timeout;
        loop {
            if let Some(status) = me.child.try_wait().map_err(|e| AiError::Sidecar(e.to_string()))? {
                let tail = log_tail(&log_file, 15);
                return Err(AiError::Sidecar(format!("llama-server exited ({status}) before becoming ready:\n{tail}")));
            }
            if check_health(&base_url).await? {
                return Ok(me);
            }
            if Instant::now() > deadline {
                me.stop().await;
                return Err(AiError::Sidecar(format!("llama-server did not become ready within {}s; see {}", ready_timeout.as_secs(), log_file.display())));
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    /// Whether the process is still alive.
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub async fn stop(&mut self) {
        let _ = self.child.kill().await;
    }

    /// The embedder for a server started with [`SidecarOptions::embedding`]; `model_id` (the
    /// catalog id) keeps the index valid across restarts on other ports.
    pub fn embedder(&self, model_id: &str) -> crate::embed::HttpEmbedder {
        crate::embed::HttpEmbedder::new(
            crate::embed::Api::OpenAi,
            &format!("{}/v1", self.info.base_url),
            &self.info.model,
            Some(self.api_key.clone()),
            true,
            Some(format!("managed|{model_id}")),
        )
        .expect("loopback client builds")
    }

    /// An OpenAI-compatible provider config for this server (its API key goes with it).
    pub fn provider(&self) -> crate::provider::Provider {
        let mut cfg = crate::provider::ProviderConfig::new(
            MANAGED_PROVIDER_ID,
            "Managed llama.cpp",
            crate::provider::ProviderKind::OpenAiCompatible,
            format!("{}/v1", self.info.base_url),
            self.info.model.clone(),
        );
        cfg.has_api_key = true;
        crate::provider::Provider::new(cfg, Some(self.api_key.clone())).expect("loopback client builds")
    }
}

/// Provider id of the managed sidecar (not stored in `ai.json`).
pub const MANAGED_PROVIDER_ID: &str = "managed-llama";

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `GET /health`: 200 `{"status":"ok"}` once the model is loaded (503 while loading).
pub async fn check_health(base_url: &str) -> Result<bool> {
    let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().map_err(|e| AiError::Sidecar(e.to_string()))?;
    match client.get(format!("{base_url}/health")).send().await {
        Ok(r) if r.status().is_success() => {
            let body = r.text().await.unwrap_or_default();
            Ok(body.is_empty() || body.contains("ok"))
        }
        Ok(_) | Err(_) => Ok(false),
    }
}

fn log_tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn args_bind_loopback_with_a_key() {
        let a = server_args(Path::new("/m/Qwen3.5-4B-Q4_K_M.gguf"), 41234, "devall-k", &SidecarOptions::default());
        let s = a.join(" ");
        assert!(s.contains("--host 127.0.0.1 --port 41234 --api-key devall-k"));
        assert!(s.contains("--alias Qwen3.5-4B-Q4_K_M"));
        assert!(s.contains("-ngl 999") && s.contains("--no-webui") && s.contains("--jinja"));
        assert!(!s.contains("fim-qwen"), "never use upstream Qwen2.5 FIM presets");
        let cpu = server_args(Path::new("m.gguf"), 1, "k", &SidecarOptions { gpu: false, ..Default::default() });
        assert!(!cpu.contains(&"-ngl".to_string()));
        assert!(random_key().len() > 40);
    }

    #[test]
    fn extracts_zip_and_finds_server() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("llama-test-bin-win-cpu-x64.zip");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file(server_exe_name(), o).unwrap();
            z.write_all(b"fake").unwrap();
            z.start_file("ggml-base.dll", o).unwrap();
            z.write_all(b"dll").unwrap();
            z.start_file("../escape.txt", o).unwrap();
            z.write_all(b"nope").unwrap();
            z.finish().unwrap();
        }
        let dest = dir.path().join("out");
        extract(&archive, &dest).unwrap();
        assert!(find_file(&dest, server_exe_name(), 3).is_some());
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn extracts_tar_gz_with_nested_dir() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("llama-test-bin-macos-arm64.tar.gz");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut t = tar::Builder::new(gz);
            let data = b"#!/bin/sh\n";
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            t.append_data(&mut h, format!("build/bin/{}", server_exe_name()), &data[..]).unwrap();
            t.into_inner().unwrap().finish().unwrap();
        }
        let dest = dir.path().join("out");
        extract(&archive, &dest).unwrap();
        let found = find_file(&dest, server_exe_name(), 4).unwrap();
        assert!(found.ends_with(Path::new("build").join("bin").join(server_exe_name())));
    }

    #[tokio::test]
    async fn health_polling() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut n = 0;
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                n += 1;
                let resp = if n == 1 {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 37\r\nConnection: close\r\n\r\n{\"error\":{\"message\":\"Loading model\"}}"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}"
                };
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        let base = format!("http://{addr}");
        assert!(!check_health(&base).await.unwrap());
        assert!(check_health(&base).await.unwrap());
        assert!(!check_health("http://127.0.0.1:1").await.unwrap());
    }

    #[test]
    fn resolves_installed_builds() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_server(dir.path(), None, None, None).is_none() || std::env::var(SERVER_ENV).is_ok());
        let inst_dir = install_dir(dir.path(), LlamaVariant::WinCpuX64);
        std::fs::create_dir_all(&inst_dir).unwrap();
        let exe = inst_dir.join(server_exe_name());
        std::fs::write(&exe, b"x").unwrap();
        let marker = Installed { build: LLAMA_CPP_BUILD.into(), variant: LlamaVariant::WinCpuX64, server: exe.clone() };
        std::fs::write(inst_dir.join(MARKER), serde_json::to_vec(&marker).unwrap()).unwrap();
        assert_eq!(installed(dir.path()), vec![marker]);
        if std::env::var(SERVER_ENV).is_err() {
            assert_eq!(resolve_server(dir.path(), None, None, Some(LlamaVariant::WinCpuX64)), Some(exe));
        }
    }
}

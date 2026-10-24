//! Local semantic search over a project: notes, saved requests, mock stubs and environments are
//! chunked, embedded with a local model ([`crate::embed`]) and stored in
//! `<project data>/embeddings/` (on the device, outside the repository — see
//! `devall_core::local::project_dir`):
//!
//! * `index.json` — format version, embedder id, dimensions and one entry per chunk (kind,
//!   source, title, the embedded text, its content hash);
//! * `vectors.f32` — the L2-normalized vectors, little-endian `f32`, in chunk order.
//!
//! Builds are incremental: a chunk whose content hash (and embedder) is unchanged keeps its
//! vector, so re-indexing after editing one note embeds only that note's chunks. Search ranks by
//! cosine similarity fused (reciprocal rank fusion) with a BM25 keyword rank over the same
//! chunks; without a reachable embedder it falls back to keywords alone.
//!
//! What is embedded: notes by heading and paragraph; requests by name, method, URL and docs;
//! mocks by stub name, method/URL and a short response summary; environments by *names only*
//! (environment, variable and secret names — never values). Every chunk passes through the
//! project [`Redactor`] first.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use devall_core::Project;
use devall_core::notes::{strip_front_matter, title_of};
use devall_core::tree::TreeNode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Redactor;
use crate::download::CancelFlag;
use crate::embed::Embed;
use crate::{AiError, Result};

/// Folder of the index inside the project data folder.
pub const INDEX_DIR: &str = "embeddings";
const INDEX_FILE: &str = "index.json";
const VECTORS_FILE: &str = "vectors.f32";
const INDEX_VERSION: u32 = 1;
/// Characters per chunk (≈ 300–400 tokens: well inside every embedding model's context).
const CHUNK_CHARS: usize = 1_400;
/// Inputs per embedding call.
const BATCH: usize = 16;
/// Reciprocal-rank-fusion constant.
const RRF_K: f32 = 60.0;

/// One searchable piece of the project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    /// `note`, `request`, `mock`, `environment`.
    pub kind: String,
    /// Project-relative file: `notes/a.md`, `http/users/list.req.yaml`, `mocks/mappings/x.json`,
    /// `environments/staging.env.yaml`.
    pub source: String,
    pub title: String,
    /// Note heading, request method, mock stub id…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// The text that was embedded (already redacted).
    pub text: String,
    /// SHA-256 of kind, title and text.
    pub hash: String,
}

impl Chunk {
    fn new(kind: &str, source: &str, title: &str, anchor: Option<String>, text: String) -> Self {
        let mut h = Sha256::new();
        for part in [kind, title, anchor.as_deref().unwrap_or(""), &text] {
            h.update(part.as_bytes());
            h.update([0]);
        }
        Self { kind: kind.into(), source: source.into(), title: title.into(), anchor, text, hash: hex::encode(h.finalize()) }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexFile {
    version: u32,
    embedder: String,
    #[serde(default)]
    embedder_label: String,
    dims: usize,
    built_at: String,
    #[serde(default)]
    partial: bool,
    chunks: Vec<Chunk>,
}

/// An index loaded from disk.
pub struct Index {
    pub embedder: String,
    pub embedder_label: String,
    pub dims: usize,
    pub built_at: String,
    pub partial: bool,
    pub chunks: Vec<Chunk>,
    vectors: Vec<f32>,
}

impl Index {
    pub fn vector(&self, i: usize) -> &[f32] {
        &self.vectors[i * self.dims..(i + 1) * self.dims]
    }
}

pub fn index_dir(root: &Path) -> PathBuf {
    devall_core::local::project_dir(root).join(INDEX_DIR)
}

/// Loads `<project data>/embeddings/` (`None` when there is no usable index).
pub fn load(root: &Path) -> Option<Index> {
    let dir = index_dir(root);
    let meta: IndexFile = serde_json::from_slice(&std::fs::read(dir.join(INDEX_FILE)).ok()?).ok()?;
    if meta.version != INDEX_VERSION || meta.dims == 0 {
        return None;
    }
    let raw = std::fs::read(dir.join(VECTORS_FILE)).ok()?;
    if raw.len() != meta.chunks.len() * meta.dims * 4 {
        return None;
    }
    let vectors = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    Some(Index { embedder: meta.embedder, embedder_label: meta.embedder_label, dims: meta.dims, built_at: meta.built_at, partial: meta.partial, chunks: meta.chunks, vectors })
}

fn save(root: &Path, meta: &IndexFile, vectors: &[f32]) -> Result<()> {
    let dir = index_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| AiError::Io { path: dir.clone(), source: e })?;
    let mut bytes = Vec::with_capacity(vectors.len() * 4);
    for v in vectors {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    // Vectors first: a crash in between leaves a length mismatch, which `load` rejects.
    devall_core::fsutil::atomic_write(&dir.join(VECTORS_FILE), &bytes)?;
    let json = serde_json::to_vec(meta).map_err(|e| AiError::Invalid(e.to_string()))?;
    devall_core::fsutil::atomic_write(&dir.join(INDEX_FILE), &json)?;
    Ok(())
}

/// Deletes the index (it is a cache; rebuilding recreates it).
pub fn clear(root: &Path) -> Result<()> {
    let dir = index_dir(root);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| AiError::Io { path: dir, source: e })?;
    }
    Ok(())
}

fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// ---------- chunking ----------

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect() }
}

/// Splits Markdown into `(heading, text)` chunks: sections by heading, long sections by paragraph.
pub fn chunk_markdown(body: &str) -> Vec<(Option<String>, String)> {
    let mut sections: Vec<(Option<String>, Vec<String>)> = vec![(None, Vec::new())];
    let mut para = String::new();
    let mut fence = false;
    let flush_para = |para: &mut String, sections: &mut Vec<(Option<String>, Vec<String>)>| {
        let p = para.trim();
        if !p.is_empty() {
            sections.last_mut().expect("one section").1.push(p.to_string());
        }
        para.clear();
    };
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        if !fence && t.starts_with('#') && t.trim_start_matches('#').starts_with(' ') {
            flush_para(&mut para, &mut sections);
            sections.push((Some(t.trim_start_matches('#').trim().to_string()), Vec::new()));
            continue;
        }
        if !fence && t.is_empty() {
            flush_para(&mut para, &mut sections);
            continue;
        }
        para.push_str(line);
        para.push('\n');
    }
    flush_para(&mut para, &mut sections);
    let mut out = Vec::new();
    for (heading, paras) in sections {
        let mut cur = String::new();
        for p in paras {
            if !cur.is_empty() && cur.chars().count() + p.chars().count() > CHUNK_CHARS {
                out.push((heading.clone(), std::mem::take(&mut cur)));
            }
            if !cur.is_empty() {
                cur.push_str("\n\n");
            }
            // A single huge paragraph is cut into pieces.
            let mut p = p.as_str();
            while p.chars().count() > CHUNK_CHARS {
                let cut = p.char_indices().nth(CHUNK_CHARS).map(|(i, _)| i).unwrap_or(p.len());
                let cut = p[..cut].rfind(char::is_whitespace).filter(|i| *i > CHUNK_CHARS / 2).unwrap_or(cut);
                if !cur.is_empty() {
                    out.push((heading.clone(), std::mem::take(&mut cur)));
                }
                out.push((heading.clone(), p[..cut].trim().to_string()));
                p = p[cut..].trim_start();
            }
            cur.push_str(p);
        }
        if !cur.trim().is_empty() {
            out.push((heading.clone(), cur));
        } else if let Some(h) = heading {
            // A heading with no body still says what the note covers.
            out.push((Some(h.clone()), h));
        }
    }
    out
}

fn mock_files(root: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("mocks").join("mappings"), &mut out);
    out.sort();
    out
}

/// A stub's response, summarised: status plus the start of its body (or body file name).
fn response_summary(r: &Value) -> String {
    let status = r["status"].as_u64().unwrap_or(200);
    let body = if let Some(s) = r["body"].as_str() {
        s.to_string()
    } else if !r["jsonBody"].is_null() {
        r["jsonBody"].to_string()
    } else if let Some(f) = r["bodyFileName"].as_str() {
        format!("(body file {f})")
    } else if let Some(p) = r["proxyBaseUrl"].as_str() {
        format!("(proxied to {p})")
    } else {
        String::new()
    };
    let body = cap(body.trim(), 300);
    if body.is_empty() { format!("→ {status}") } else { format!("→ {status} {body}") }
}

/// Every chunk of the project, redacted with `redactor` (secret values and credential patterns).
pub fn collect_chunks(project: &Project, redactor: &Redactor) -> Result<Vec<Chunk>> {
    let clean = |s: &str| redactor.redact(s, true).0;
    let mut out = Vec::new();
    // Notes.
    for (path, text) in project.all_notes()? {
        let title = title_of(&text, &path);
        let (_, body) = strip_front_matter(&text);
        for (heading, part) in chunk_markdown(body) {
            let text = clean(&match &heading {
                Some(h) if h != &title => format!("{h}\n{part}"),
                _ => part,
            });
            out.push(Chunk::new("note", &format!("notes/{path}"), &title, heading, text));
        }
    }
    // Requests.
    fn walk(n: &TreeNode, folder: &str, out: &mut Vec<(String, String, String, String, String)>) {
        match n {
            TreeNode::Folder { name, children, path } => {
                let f = if path.is_empty() { String::new() } else if folder.is_empty() { name.clone() } else { format!("{folder}/{name}") };
                for c in children {
                    walk(c, &f, out);
                }
            }
            TreeNode::Request { name, method, path, protocol, .. } => out.push((path.clone(), name.clone(), method.clone(), protocol.clone(), folder.to_string())),
        }
    }
    let mut reqs = Vec::new();
    walk(&project.tree()?, "", &mut reqs);
    for (path, name, method, protocol, folder) in reqs {
        let mut text = format!("{method} {name}");
        if !folder.is_empty() {
            text.push_str(&format!("\nFolder: {folder}"));
        }
        if protocol == "http"
            && let Ok(r) = project.read_request(&path)
        {
            text.push_str(&format!("\n{}", r.url));
            if let Some(d) = r.docs.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                text.push_str(&format!("\n{}", cap(d, CHUNK_CHARS)));
            }
        } else if protocol != "http" {
            text.push_str(&format!("\n({protocol})"));
        }
        out.push(Chunk::new("request", &format!("http/{path}"), &name, Some(method), clean(&text)));
    }
    // Mock stubs.
    let root = project.root();
    for f in mock_files(root) {
        let rel = f.strip_prefix(root).unwrap_or(&f).to_string_lossy().replace('\\', "/");
        let Ok(v) = std::fs::read(&f).map_err(|_| ()).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|_| ())) else { continue };
        let mappings = match v.get("mappings").and_then(Value::as_array) {
            Some(list) => list.clone(),
            None => vec![v],
        };
        for m in mappings {
            let rq = &m["request"];
            let method = rq["method"].as_str().unwrap_or("ANY");
            let url = ["url", "urlPath", "urlPattern", "urlPathPattern", "urlPathTemplate"].iter().find_map(|k| rq[*k].as_str()).unwrap_or("(any URL)");
            let name = m["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("{method} {url}"));
            let id = m["id"].as_str().or(m["uuid"].as_str()).map(str::to_string);
            let text = format!("Mock stub: {name}\n{method} {url} {}", response_summary(&m["response"]));
            out.push(Chunk::new("mock", &rel, &name, id, clean(&text)));
        }
    }
    // Environments: names only.
    for e in project.environments()? {
        let vars = e.file.variables.keys().cloned().collect::<Vec<_>>().join(", ");
        let secrets = e.file.secrets.join(", ");
        let mut text = format!("Environment {} ({})", e.file.name, e.id);
        if e.file.protected {
            text.push_str(" — protected");
        }
        if !vars.is_empty() {
            text.push_str(&format!("\nVariables: {vars}"));
        }
        if !secrets.is_empty() {
            text.push_str(&format!("\nSecrets: {secrets}"));
        }
        out.push(Chunk::new("environment", &format!("environments/{}.env.yaml", e.id), &e.file.name, None, clean(&text)));
    }
    Ok(out)
}

// ---------- status / build ----------

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedder_label: Option<String>,
    pub dims: usize,
    pub chunks: usize,
    /// Chunks per kind.
    pub by_kind: BTreeMap<String, usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_at: Option<String>,
    /// A build was cancelled: some chunks are missing.
    pub partial: bool,
    /// Chunks that are new or changed since the last build (when the project was scanned).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<usize>,
    /// Chunks in the index that no longer exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<usize>,
    pub size_bytes: u64,
}

/// What the index holds, and (with `current`) how far it is behind the project.
pub fn status(root: &Path, current: Option<&[Chunk]>) -> IndexStatus {
    let Some(ix) = load(root) else {
        return IndexStatus { stale: current.map(<[Chunk]>::len), ..Default::default() };
    };
    let mut by_kind = BTreeMap::new();
    for c in &ix.chunks {
        *by_kind.entry(c.kind.clone()).or_insert(0) += 1;
    }
    let have: HashSet<&str> = ix.chunks.iter().map(|c| c.hash.as_str()).collect();
    let (stale, removed) = match current {
        Some(cur) => {
            let now: HashSet<&str> = cur.iter().map(|c| c.hash.as_str()).collect();
            (Some(cur.iter().filter(|c| !have.contains(c.hash.as_str())).count()), Some(have.iter().filter(|h| !now.contains(*h)).count()))
        }
        None => (None, None),
    };
    let dir = index_dir(root);
    let size_bytes = [INDEX_FILE, VECTORS_FILE].iter().filter_map(|f| std::fs::metadata(dir.join(f)).ok()).map(|m| m.len()).sum();
    IndexStatus {
        exists: true,
        embedder: Some(ix.embedder),
        embedder_label: Some(ix.embedder_label).filter(|l| !l.is_empty()),
        dims: ix.dims,
        chunks: ix.chunks.len(),
        by_kind,
        built_at: Some(ix.built_at),
        partial: ix.partial,
        stale,
        removed,
        size_bytes,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BuildProgress {
    /// The project was scanned: `total` chunks, `reused` of them unchanged.
    Scanned { total: usize, reused: usize, to_embed: usize },
    Embedded { done: usize, to_embed: usize },
    Saved { chunks: usize },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildOutcome {
    pub status: IndexStatus,
    pub embedded: usize,
    pub reused: usize,
    pub cancelled: bool,
}

/// Builds or refreshes the index: unchanged chunks keep their vectors, new or changed ones are
/// embedded in batches. Cancelling keeps what was embedded so far (the next build continues).
pub async fn build(root: &Path, chunks: Vec<Chunk>, embedder: &dyn Embed, cancel: &CancelFlag, mut progress: impl FnMut(BuildProgress)) -> Result<BuildOutcome> {
    let id = embedder.id();
    let mut known: HashMap<String, Vec<f32>> = HashMap::new();
    if let Some(old) = load(root).filter(|ix| ix.embedder == id) {
        for (i, c) in old.chunks.iter().enumerate() {
            known.entry(c.hash.clone()).or_insert_with(|| old.vector(i).to_vec());
        }
    }
    let mut vectors: Vec<Option<Vec<f32>>> = chunks.iter().map(|c| known.get(&c.hash).cloned()).collect();
    let reused = vectors.iter().filter(|v| v.is_some()).count();
    let pending: Vec<usize> = (0..chunks.len()).filter(|i| vectors[*i].is_none()).collect();
    progress(BuildProgress::Scanned { total: chunks.len(), reused, to_embed: pending.len() });
    let mut dims = vectors.iter().flatten().map(Vec::len).next().unwrap_or(0);
    let mut embedded = 0usize;
    let mut cancelled = false;
    for batch in pending.chunks(BATCH) {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let inputs: Vec<String> = batch.iter().map(|i| embedder.document_text(&chunks[*i].title, &chunks[*i].text)).collect();
        let out = embedder.embed(&inputs).await?;
        for (i, mut v) in batch.iter().zip(out) {
            if dims == 0 {
                dims = v.len();
            }
            if v.len() != dims {
                return Err(AiError::Protocol(format!("the embedding model returned {} dimensions, expected {dims}", v.len())));
            }
            normalize(&mut v);
            vectors[*i] = Some(v);
        }
        embedded += batch.len();
        progress(BuildProgress::Embedded { done: embedded, to_embed: pending.len() });
    }
    let mut kept_chunks = Vec::with_capacity(chunks.len());
    let mut flat = Vec::with_capacity(chunks.len() * dims);
    for (c, v) in chunks.into_iter().zip(vectors) {
        if let Some(v) = v {
            flat.extend_from_slice(&v);
            kept_chunks.push(c);
        }
    }
    let meta = IndexFile {
        version: INDEX_VERSION,
        embedder: id,
        embedder_label: embedder.label(),
        dims,
        built_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        partial: cancelled,
        chunks: kept_chunks,
    };
    if dims > 0 || meta.chunks.is_empty() {
        save(root, &meta, &flat)?;
    }
    progress(BuildProgress::Saved { chunks: meta.chunks.len() });
    Ok(BuildOutcome { status: status(root, None), embedded, reused, cancelled })
}

// ---------- search ----------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub kind: String,
    pub source: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// The start of the matched chunk.
    pub snippet: String,
    /// The whole chunk (for prompts).
    #[serde(skip)]
    pub text: String,
    /// Fused score (higher is better).
    pub score: f32,
    /// Cosine similarity, when the query was embedded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchMode {
    /// Vector similarity fused with keywords.
    Hybrid,
    /// Keywords only (no embedder available, or an index built with another model).
    Keyword,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub mode: SearchMode,
    pub hits: Vec<SearchHit>,
    /// Why the search fell back to keywords, or that there is no index yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(|w| w.trim_end_matches('s').to_string()).filter(|w| !w.is_empty()).collect()
}

/// BM25 scores of `docs` for `query` (k1 = 1.2, b = 0.75).
pub fn bm25(docs: &[&str], query: &str) -> Vec<f32> {
    let q: Vec<String> = {
        let mut seen = HashSet::new();
        tokens(query).into_iter().filter(|t| seen.insert(t.clone())).collect()
    };
    let toks: Vec<Vec<String>> = docs.iter().map(|d| tokens(d)).collect();
    let n = docs.len().max(1) as f32;
    let avg = (toks.iter().map(Vec::len).sum::<usize>() as f32 / n).max(1.0);
    let df: Vec<f32> = q.iter().map(|t| toks.iter().filter(|d| d.contains(t)).count() as f32).collect();
    toks.iter()
        .map(|d| {
            let len = d.len() as f32;
            q.iter()
                .zip(&df)
                .map(|(t, df)| {
                    let tf = d.iter().filter(|w| *w == t).count() as f32;
                    if tf == 0.0 {
                        return 0.0;
                    }
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    idf * tf * 2.2 / (tf + 1.2 * (0.25 + 0.75 * len / avg))
                })
                .sum()
        })
        .collect()
}

fn snippet(text: &str) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= 220 { one } else { format!("{}…", one.chars().take(220).collect::<String>()) }
}

/// Ranks `chunks` for `query`: cosine similarity with `query_vec` (when given) fused with BM25.
pub fn rank(chunks: &[Chunk], vectors: Option<(&Index, &[f32])>, query: &str, limit: usize) -> Vec<SearchHit> {
    let texts: Vec<String> = chunks.iter().map(|c| format!("{} {} {} {}", c.title, c.anchor.as_deref().unwrap_or(""), c.source, c.text)).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let kw = bm25(&refs, query);
    let mut fused = vec![0f32; chunks.len()];
    let mut order: Vec<usize> = (0..chunks.len()).filter(|i| kw[*i] > 0.0).collect();
    order.sort_by(|a, b| kw[*b].total_cmp(&kw[*a]));
    for (r, i) in order.iter().enumerate() {
        fused[*i] += 1.0 / (RRF_K + r as f32 + 1.0);
    }
    let mut sims: Vec<Option<f32>> = vec![None; chunks.len()];
    if let Some((ix, qv)) = vectors {
        let mut by_sim: Vec<(usize, f32)> = (0..chunks.len()).map(|i| (i, dot(ix.vector(i), qv))).collect();
        by_sim.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (r, (i, s)) in by_sim.iter().enumerate() {
            sims[*i] = Some(*s);
            // Weakly related chunks don't get a vector vote.
            if *s > 0.05 {
                fused[*i] += 1.0 / (RRF_K + r as f32 + 1.0);
            }
        }
    }
    let mut idx: Vec<usize> = (0..chunks.len()).filter(|i| fused[*i] > 0.0).collect();
    idx.sort_by(|a, b| fused[*b].total_cmp(&fused[*a]));
    idx.truncate(limit);
    idx.into_iter()
        .map(|i| {
            let c = &chunks[i];
            SearchHit {
                kind: c.kind.clone(),
                source: c.source.clone(),
                title: c.title.clone(),
                anchor: c.anchor.clone(),
                snippet: snippet(&c.text),
                text: c.text.clone(),
                score: (fused[i] * 10_000.0).round() / 10_000.0,
                similarity: sims[i].map(|s| (s * 1000.0).round() / 1000.0),
            }
        })
        .collect()
}

/// Searches the project's index. `embedder` embeds the query when it matches the index's model;
/// otherwise (or without an index) the search runs on keywords over the indexed — or freshly
/// collected — chunks.
pub async fn search(root: &Path, query: &str, limit: usize, embedder: Option<&dyn Embed>, fallback_chunks: impl FnOnce() -> Result<Vec<Chunk>>) -> Result<SearchResult> {
    let limit = limit.clamp(1, 100);
    if query.trim().is_empty() {
        return Ok(SearchResult { mode: SearchMode::Keyword, hits: Vec::new(), note: None });
    }
    let Some(ix) = load(root) else {
        let chunks = fallback_chunks()?;
        return Ok(SearchResult {
            mode: SearchMode::Keyword,
            hits: rank(&chunks, None, query, limit),
            note: Some("No semantic index yet — keyword results. Build it in AI → Search for meaning-based results.".into()),
        });
    };
    let (qv, note) = match embedder {
        Some(e) if e.id() == ix.embedder => match e.embed(&[e.query_text(query.trim())]).await {
            Ok(mut v) if v.first().is_some_and(|x| x.len() == ix.dims) => {
                let mut q = v.swap_remove(0);
                normalize(&mut q);
                (Some(q), None)
            }
            Ok(_) => (None, Some("The embedding model returned unexpected vectors — keyword results.".to_string())),
            Err(e) => (None, Some(format!("Embedding the query failed ({e}) — keyword results."))),
        },
        Some(e) => (None, Some(format!("The index was built with {}; the current embedding model is {} — keyword results until the index is rebuilt.", label_of(&ix), e.label()))),
        None => (None, Some(format!("The embedding model ({}) isn't available — keyword results.", label_of(&ix)))),
    };
    let hits = rank(&ix.chunks, qv.as_deref().map(|q| (&ix, q)), query, limit);
    Ok(SearchResult { mode: if qv.is_some() { SearchMode::Hybrid } else { SearchMode::Keyword }, hits, note })
}

fn label_of(ix: &Index) -> String {
    if ix.embedder_label.is_empty() { ix.embedder.clone() } else { ix.embedder_label.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::FakeEmbedder;

    fn project() -> (tempfile::TempDir, Project) {
        devall_core::local::isolate_for_tests();
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Shop").unwrap();
        let n = p.create_note("", "Payments").unwrap();
        p.save_note(&n, "# Payments\n\nWe charge cards through Stripe.\n\n## Refunds\n\nRefunds are issued with the refund endpoint within 30 days.\n").unwrap();
        let a = p.create_note("", "Auth").unwrap();
        p.save_note(&a, "# Auth\n\nThe SPA signs in with PKCE. Tokens rotate hourly.\n").unwrap();
        let mut r = devall_core::HttpRequestFile::new("Create refund");
        r.method = "POST".into();
        r.url = "{{baseUrl}}/refunds".into();
        r.docs = Some("Issues a refund for a charge.".into());
        p.create_request("", r).unwrap();
        std::fs::create_dir_all(dir.path().join("mocks/mappings")).unwrap();
        std::fs::write(
            dir.path().join("mocks/mappings/charges.json"),
            r#"{"name":"List charges","request":{"method":"GET","urlPath":"/charges"},"response":{"status":200,"jsonBody":{"data":[]}}}"#,
        )
        .unwrap();
        let mut env = devall_core::EnvironmentFile::new("Staging");
        env.variables.insert("baseUrl".into(), "https://staging.example.com".into());
        env.secrets.push("STRIPE_KEY".into());
        p.save_environment("staging", &env).unwrap();
        (dir, p)
    }

    #[test]
    fn markdown_chunks_follow_headings() {
        let c = chunk_markdown("intro line\n\n# A\n\npara one\n\n```\n# not a heading\n\ncode\n```\n\n## B\n\n");
        assert_eq!(c[0], (None, "intro line".to_string()));
        assert_eq!(c[1].0.as_deref(), Some("A"));
        assert!(c[1].1.contains("# not a heading") && c[1].1.contains("code"), "fences stay together: {c:?}");
        assert_eq!(c[2], (Some("B".into()), "B".into()));
        let long = format!("# Big\n\n{}", "word ".repeat(1000));
        let parts = chunk_markdown(&long);
        assert!(parts.len() >= 3 && parts.iter().all(|(_, t)| t.chars().count() <= CHUNK_CHARS), "{:?}", parts.iter().map(|p| p.1.len()).collect::<Vec<_>>());
    }

    #[test]
    fn chunks_cover_notes_requests_mocks_and_env_names_only() {
        let (_d, p) = project();
        let chunks = collect_chunks(&p, &Redactor::default()).unwrap();
        let kinds: Vec<&str> = chunks.iter().map(|c| c.kind.as_str()).collect();
        assert!(kinds.contains(&"note") && kinds.contains(&"request") && kinds.contains(&"mock") && kinds.contains(&"environment"), "{kinds:?}");
        let env = chunks.iter().find(|c| c.source == "environments/staging.env.yaml").unwrap();
        assert!(env.text.contains("baseUrl") && env.text.contains("STRIPE_KEY"));
        assert!(!env.text.contains("staging.example.com"), "variable values are never indexed");
        let req = chunks.iter().find(|c| c.kind == "request").unwrap();
        assert!(req.text.contains("POST Create refund") && req.text.contains("/refunds") && req.text.contains("Issues a refund"));
        let mock = chunks.iter().find(|c| c.kind == "mock").unwrap();
        assert!(mock.text.contains("GET /charges") && mock.source == "mocks/mappings/charges.json");
        let refunds = chunks.iter().find(|c| c.anchor.as_deref() == Some("Refunds")).unwrap();
        assert_eq!(refunds.source, "notes/payments.md");
        // Secret values are masked before anything is embedded.
        let red = Redactor::new(["Stripe".to_string()].into_iter().collect());
        assert!(collect_chunks(&p, &red).unwrap().iter().all(|c| !c.text.contains("Stripe")));
    }

    #[tokio::test]
    async fn builds_incrementally_and_searches() {
        let (dir, p) = project();
        let fake = FakeEmbedder::new(64);
        let cancel = CancelFlag::default();
        let chunks = collect_chunks(&p, &Redactor::default()).unwrap();
        let n = chunks.len();
        let mut events = Vec::new();
        let out = build(dir.path(), chunks, &fake, &cancel, |e| events.push(e)).await.unwrap();
        assert_eq!((out.embedded, out.reused, out.status.chunks), (n, 0, n));
        assert!(matches!(events.last(), Some(BuildProgress::Saved { .. })));
        assert!(index_dir(dir.path()).join("vectors.f32").is_file());
        assert!(!dir.path().join(".devall").exists(), "the index lives outside the repository");

        // Editing one note re-embeds only its chunks.
        let before = fake.embedded.load(Ordering::SeqCst);
        p.save_note("auth.md", "# Auth\n\nThe SPA signs in with PKCE and refresh tokens.\n").unwrap();
        let chunks = collect_chunks(&p, &Redactor::default()).unwrap();
        let st = status(dir.path(), Some(&chunks));
        assert_eq!((st.stale, st.removed), (Some(1), Some(1)));
        let out = build(dir.path(), chunks, &fake, &cancel, |_| {}).await.unwrap();
        assert_eq!(out.embedded, 1);
        assert_eq!(fake.embedded.load(Ordering::SeqCst) - before, 1);

        // Hybrid search finds the refunds note and request; keyword fallback without an embedder.
        let r = search(dir.path(), "how do refunds work", 5, Some(&fake), || Ok(vec![])).await.unwrap();
        assert_eq!(r.mode, SearchMode::Hybrid);
        assert!(r.hits.iter().take(3).any(|h| h.source == "notes/payments.md" && h.anchor.as_deref() == Some("Refunds")), "{:?}", r.hits);
        assert!(r.hits.iter().any(|h| h.kind == "request"));
        let k = search(dir.path(), "refunds", 5, None, || Ok(vec![])).await.unwrap();
        assert_eq!(k.mode, SearchMode::Keyword);
        assert!(k.note.is_some() && !k.hits.is_empty());
        let other = FakeEmbedder::new(32);
        let m = search(dir.path(), "refunds", 5, Some(&other), || Ok(vec![])).await.unwrap();
        assert_eq!(m.mode, SearchMode::Keyword, "an index from another model is not mixed with this one");

        // Cancelling keeps what was embedded; the next build continues.
        clear(dir.path()).unwrap();
        cancel.store(true, Ordering::SeqCst);
        let chunks = collect_chunks(&p, &Redactor::default()).unwrap();
        let out = build(dir.path(), chunks.clone(), &fake, &cancel, |_| {}).await.unwrap();
        assert!(out.cancelled && out.status.chunks == 0);
        cancel.store(false, Ordering::SeqCst);
        let out = build(dir.path(), chunks, &fake, &cancel, |_| {}).await.unwrap();
        assert!(!out.cancelled && !out.status.partial && out.status.chunks == n);
    }

    #[test]
    fn bm25_prefers_rare_terms() {
        let s = bm25(&["the cat sat", "the dog sat", "the the the"], "cat the");
        assert!(s[0] > s[1] && s[0] > s[2]);
        assert_eq!(bm25(&["a b"], "zzz"), vec![0.0]);
    }
}

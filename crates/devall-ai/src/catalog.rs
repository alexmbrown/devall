//! Curated one-click models (Apache-2.0 only) and the pinned llama.cpp release.
//!
//! Lineup per `research_notes/Developer toolkit competitive research/local_models_update_oct2026.md`
//! §7 (Oct 2026). **No Qwen2.5-family model is ever listed** (a test enforces it); llama.cpp's
//! `--fim-qwen-{1.5b,3b,7b,14b}` presets still load Qwen2.5 Coder, so the sidecar never uses them.
//! File names, sizes and SHA-256 digests were read from the Hugging Face API (`/api/models/<repo>
//! /tree/main`, LFS `oid`) and the GitHub release API on 2026-10-21. Re-validate each quarter.

use serde::Serialize;

use crate::hardware::{LlamaVariant, Tier};

/// llama.cpp build bundled/downloaded by this release.
pub const LLAMA_CPP_BUILD: &str = "b11796";
pub const LLAMA_CPP_BUILD_NUMBER: u32 = 11796;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelRole {
    /// Chat, Q&A, structured JSON, tool calling.
    General,
    /// Code chat and fill-in-the-middle completion.
    Code,
    /// Inline autocomplete (FIM) only.
    Fim,
    Embedding,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: &'static str,
    pub name: &'static str,
    pub family: &'static str,
    pub role: ModelRole,
    pub license: &'static str,
    /// Hugging Face repository and file (`https://huggingface.co/<repo>/resolve/main/<file>`).
    pub hf_repo: &'static str,
    pub hf_file: &'static str,
    pub size_bytes: u64,
    /// LFS SHA-256 of the file.
    pub sha256: &'static str,
    pub quant: &'static str,
    /// Context the sidecar starts with (the model may support more).
    pub context: u32,
    /// Tiers this file fits comfortably.
    pub tiers: &'static [Tier],
    /// Supports a thinking toggle (sent as `enable_thinking=false`).
    pub thinking: bool,
    /// Documented fill-in-the-middle support.
    pub fim: bool,
    /// Equivalent Ollama tag, when Ollama is the runtime of choice.
    pub ollama_tag: Option<&'static str>,
    pub min_llama_cpp_build: u32,
    pub notes: &'static str,
}

impl CatalogModel {
    pub fn download_url(&self) -> String {
        format!("https://huggingface.co/{}/resolve/main/{}", self.hf_repo, self.hf_file)
    }
}

use Tier::*;

/// The curated list, general models first.
pub fn models() -> Vec<CatalogModel> {
    let m = |id, name, family, role, hf_repo, hf_file, size_bytes, sha256, quant, tiers, thinking, fim, ollama_tag, notes| CatalogModel {
        id,
        name,
        family,
        role,
        license: "Apache-2.0",
        hf_repo,
        hf_file,
        size_bytes,
        sha256,
        quant,
        context: if role == ModelRole::Fim { 8192 } else { 16384 },
        tiers,
        thinking,
        fim,
        ollama_tag,
        min_llama_cpp_build: LLAMA_CPP_BUILD_NUMBER,
        notes,
    };
    vec![
        // ── General / structured JSON ──────────────────────────────────────────────────────────
        m("qwen3.5-4b", "Qwen3.5 4B", "Qwen3.5", ModelRole::General, "unsloth/Qwen3.5-4B-GGUF", "Qwen3.5-4B-Q4_K_M.gguf", 2_740_937_888,
          "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4", "Q4_K_M", &[T8, T16], true, false, Some("qwen3.5:4b"),
          "Default for ~8 GB machines. 262K context; thinking off by default here."),
        m("gemma-4-e4b", "Gemma 4 E4B (QAT)", "Gemma 4", ModelRole::General, "google/gemma-4-E4B-it-qat-q4_0-gguf", "gemma-4-E4B_q4_0-it.gguf", 5_154_941_280,
          "676c35070db6dbe52f93e9c864ee0fba4eddea94b9c875d9cb10daff453fbaee", "Q4_0 (QAT)", &[T8, T16], true, false, Some("gemma4:e4b"),
          "Google's official quantization-aware Q4_0 build; 4.5B effective parameters, 128K context."),
        m("gemma-4-12b", "Gemma 4 12B (QAT)", "Gemma 4", ModelRole::General, "google/gemma-4-12B-it-qat-q4_0-gguf", "gemma-4-12b-it-qat-q4_0.gguf", 6_975_879_296,
          "93567e57a8fe10b23569b9d9ec38cd005deedf71e29477c421a4b83f418a538b", "Q4_0 (QAT)", &[T16, T24], true, false, Some("gemma4:12b"),
          "Default for 16 GB machines: 256K context, native tool use."),
        m("qwen3.5-9b", "Qwen3.5 9B", "Qwen3.5", ModelRole::General, "unsloth/Qwen3.5-9B-GGUF", "Qwen3.5-9B-Q4_K_M.gguf", 5_680_522_464,
          "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8", "Q4_K_M", &[T16, T24], true, false, Some("qwen3.5:9b"),
          "Strong tool calling and code chat for 16 GB machines."),
        m("granite-4.2-8b", "Granite 4.2 8B", "Granite 4.2", ModelRole::General, "ibm-granite/granite-4.2-8b-GGUF", "granite-4.2-8b-Q4_K_M.gguf", 5_347_917_952,
          "16a9369d0805f80b7377d25d87f937a90c05dc04ad79173a52001e42c9aab311", "Q4_K_M", &[T16, T24], true, false, Some("granite4.2:8b"),
          "IBM's official GGUF; dense, conservative, 128K context."),
        m("qwen3.8-27b-q3", "Qwen3.8 27B (Q3)", "Qwen3.8", ModelRole::General, "unsloth/Qwen3.8-27B-GGUF", "Qwen3.8-27B-UD-Q3_K_XL.gguf", 13_146_393_504,
          "8c2a45ff85e7674ca185ec8eb6cdeab0e617ed9d8018caed0b64380eb2a67a5e", "UD-Q3_K_XL", &[T24], true, false, Some("qwen3.8:27b"),
          "Newest Qwen (Aug 2026), sized for 24 GB unified memory or 12–16 GB VRAM. Dense: slower per token than MoE models."),
        m("qwen3.8-27b", "Qwen3.8 27B", "Qwen3.8", ModelRole::General, "unsloth/Qwen3.8-27B-GGUF", "Qwen3.8-27B-UD-Q4_K_M.gguf", 16_464_440_224,
          "322e194ff79741c7baa497c240f677f54b201b0efab44ca8e50f122b39123482", "UD-Q4_K_M", &[T32], true, false, Some("qwen3.8:27b"),
          "Default for 32 GB+ unified memory or 24 GB VRAM."),
        m("gemma-4-26b-a4b", "Gemma 4 26B-A4B", "Gemma 4", ModelRole::General, "unsloth/gemma-4-26B-A4B-it-GGUF", "gemma-4-26B-A4B-it-UD-IQ4_XS.gguf", 13_597_177_568,
          "babd1e389d386352f71600765d37390f7dc993fbfad6725caccf996ffe34aecf", "UD-IQ4_XS", &[T24, T32], true, false, Some("gemma4:26b"),
          "Mixture-of-experts (3.8B active): much faster than 27B dense at similar quality."),
        m("qwen3.6-35b-a3b", "Qwen3.6 35B-A3B", "Qwen3.6", ModelRole::General, "unsloth/Qwen3.6-35B-A3B-GGUF", "Qwen3.6-35B-A3B-UD-Q4_K_M.gguf", 22_134_528_992,
          "ac0e2c1189e055faa36eff361580e79c5bd6f8e76bffb4ce547f167d53e31a61", "UD-Q4_K_M", &[T32], true, false, Some("qwen3.6:35b"),
          "Fast MoE alternative (3B active) for 32 GB+ machines."),
        // ── Code / FIM ─────────────────────────────────────────────────────────────────────────
        m("qwen3-coder-30b-a3b-q3", "Qwen3-Coder 30B-A3B (Q3)", "Qwen3-Coder", ModelRole::Code, "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF", "Qwen3-Coder-30B-A3B-Instruct-Q3_K_M.gguf", 14_711_850_144,
          "30c83da425db2324444b6a6cecaf4c410038a2ec73a78de2436879dc0316a371", "Q3_K_M", &[T24], false, true, Some("qwen3-coder:30b"),
          "FIM + code chat for 24 GB machines (tight on 24 GB Macs — use Mellum-4b there if memory is short)."),
        m("qwen3-coder-30b-a3b", "Qwen3-Coder 30B-A3B", "Qwen3-Coder", ModelRole::Code, "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF", "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf", 18_556_689_568,
          "fadc3e5f8d42bf7e894a785b05082e47daee4df26680389817e2093056f088ad", "Q4_K_M", &[T32], false, true, Some("qwen3-coder:30b"),
          "FIM officially supported; 3B active parameters keep completions fast."),
        m("mellum-4b", "Mellum 4B (DPO)", "Mellum", ModelRole::Fim, "JetBrains/Mellum-4b-dpo-all-gguf", "mellum-4b-dpo-all.Q8_0.gguf", 4_274_519_520,
          "9e7b95d3e895765ecf16ff56da0c021f4e5c1900a9a528ae2c0e8f5e5cc44684", "Q8_0", &[T8, T16, T24], false, true, None,
          "JetBrains' completion model (8K context) — the small-machine autocomplete choice. Load it only while editing on 8 GB machines."),
        // ── Embeddings ─────────────────────────────────────────────────────────────────────────
        m("embeddinggemma-2", "EmbeddingGemma 2 (270M text)", "EmbeddingGemma", ModelRole::Embedding, "ggml-org/embeddinggemma-2-GGUF", "embeddinggemma-2-Q8_0.gguf", 309_855_456,
          "2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135", "Q8_0", &[T8, T16, T24, T32], false, false, Some("embeddinggemma-2:270m"),
          "768-d (Matryoshka 512/256/128), 8K context; strong code retrieval."),
        m("qwen3-embedding-0.6b", "Qwen3-Embedding 0.6B", "Qwen3-Embedding", ModelRole::Embedding, "Qwen/Qwen3-Embedding-0.6B-GGUF", "Qwen3-Embedding-0.6B-Q8_0.gguf", 639_150_592,
          "06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439", "Q8_0", &[T24, T32], false, false, Some("qwen3-embedding:0.6b"),
          "Alternative embedder with an official GGUF."),
    ]
}

pub fn find(id: &str) -> Option<CatalogModel> {
    models().into_iter().find(|m| m.id == id)
}

/// What to download for a tier.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub tier: Tier,
    pub tier_label: &'static str,
    pub general: &'static str,
    pub alternatives: Vec<&'static str>,
    pub code: Option<&'static str>,
    pub fim: &'static str,
    pub embedding: &'static str,
    pub notes: Vec<&'static str>,
}

pub fn recommend(tier: Tier) -> Recommendation {
    let (general, alternatives, code, fim, notes): (_, Vec<&str>, _, _, Vec<&str>) = match tier {
        T8 => ("qwen3.5-4b", vec!["gemma-4-e4b"], None, "mellum-4b", vec!["Run one model at a time on 8 GB machines."]),
        T16 => ("gemma-4-12b", vec!["qwen3.5-9b", "granite-4.2-8b"], Some("qwen3.5-9b"), "mellum-4b", vec!["Gemma 4 26B-A4B (14 GB) is too tight for 16 GB."]),
        T24 => ("qwen3.8-27b-q3", vec!["gemma-4-26b-a4b"], Some("qwen3-coder-30b-a3b-q3"), "mellum-4b", vec!["27B dense decodes slower than the 3–4B-active MoE alternatives."]),
        T32 => ("qwen3.8-27b", vec!["qwen3.6-35b-a3b", "gemma-4-26b-a4b"], Some("qwen3-coder-30b-a3b"), "qwen3-coder-30b-a3b", vec![]),
    };
    Recommendation { tier, tier_label: tier.label(), general, alternatives, code, fim, embedding: "embeddinggemma-2", notes }
}

/// A downloadable llama.cpp archive pinned to [`LLAMA_CPP_BUILD`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlamaAsset {
    pub variant: LlamaVariant,
    pub file: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    /// CUDA builds also need the CUDA runtime archive, extracted into the same folder.
    pub runtime: Option<(&'static str, u64, &'static str)>,
}

impl LlamaAsset {
    pub fn url(file: &str) -> String {
        format!("https://github.com/ggml-org/llama.cpp/releases/download/{LLAMA_CPP_BUILD}/{file}")
    }

    pub fn total_bytes(&self) -> u64 {
        self.size_bytes + self.runtime.map(|r| r.1).unwrap_or(0)
    }
}

pub fn llama_assets() -> Vec<LlamaAsset> {
    use LlamaVariant::*;
    let a = |variant, file, size_bytes, sha256, runtime| LlamaAsset { variant, file, size_bytes, sha256, runtime };
    vec![
        a(MacosArm64, "llama-b11796-bin-macos-arm64.tar.gz", 16_136_199, "3be4c64b4303be0d17120400482545e70fc3c01de7fd2389150c5573f55f13fa", None),
        a(MacosX64, "llama-b11796-bin-macos-x64.tar.gz", 11_870_332, "e3a0a59d6a9be0e7eb5c403d9acc198bf7b86ed97ba993ea0898c62679743237", None),
        a(WinCpuX64, "llama-b11796-bin-win-cpu-x64.zip", 24_042_917, "d3ae3e9f395175b5a1cf8e9a82bbc92d21c1f44201dd0321f0196f8a4d2d0b69", None),
        a(WinVulkanX64, "llama-b11796-bin-win-vulkan-x64.zip", 36_839_158, "b7035131edd2dcfc4a123a9deb16d109af655ef7c5e0d283edd7323808cb78b7", None),
        a(
            WinCuda134X64,
            "llama-b11796-bin-win-cuda-13.4-x64.zip",
            159_695_876,
            "a42b883b9dc84632d5deb50a1513e7ce49796fbdfa83b1860697631c637d35d7",
            Some(("cudart-llama-bin-win-cuda-13.4-x64.zip", 423_535_356, "738f8c251ac22b70c3ae6f83a10cf222725df0395246a2cf58f32bdb85fbe668")),
        ),
        a(
            WinCuda124X64,
            "llama-b11796-bin-win-cuda-12.4-x64.zip",
            272_124_800,
            "681eef3b86a2e66b3b312cdc00786f265326975c46cd9e3fcb72022f6117b28b",
            Some(("cudart-llama-bin-win-cuda-12.4-x64.zip", 391_443_627, "8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6")),
        ),
        a(WinCpuArm64, "llama-b11796-bin-win-cpu-arm64.zip", 12_681_060, "ec3545d68b9ac36a2d324c539e0291d1b6e611ced357c1c205fee6d080aeb8b1", None),
        a(WinVulkanArm64, "llama-b11796-bin-win-vulkan-arm64.zip", 25_066_945, "98e2c15b138c45686a278cdd6866878520cffc08ec8ea9c600cd71d07c0e0d5e", None),
        a(UbuntuX64, "llama-b11796-bin-ubuntu-x64.tar.gz", 22_302_514, "70b856d40577f1cf66120f894b2674398175efbae7d9153566872c4b3c1b3d32", None),
        a(UbuntuVulkanX64, "llama-b11796-bin-ubuntu-vulkan-x64.tar.gz", 34_944_362, "f2305ac8c40642eb7466c615f7518f8dab3fe116177cf5a167133b96afc76098", None),
        a(UbuntuArm64, "llama-b11796-bin-ubuntu-arm64.tar.gz", 18_015_393, "6e8f529737e06de45a34f5956aedf074f1e95d39816c8820491986334f382187", None),
    ]
}

pub fn llama_asset(variant: LlamaVariant) -> Option<LlamaAsset> {
    llama_assets().into_iter().find(|a| a.variant == variant)
}

/// The pinned llama.cpp release as JSON, for tools outside Rust: `crates/devall-ai/llama-assets.json`
/// is read by `scripts/fetch-llama-server.mjs` (release bundling) and kept in sync by a test.
pub fn llama_assets_json() -> serde_json::Value {
    let assets: Vec<serde_json::Value> = llama_assets()
        .into_iter()
        .map(|a| {
            serde_json::json!({
                "variant": a.variant.asset_suffix(),
                "file": a.file,
                "url": LlamaAsset::url(a.file),
                "sizeBytes": a.size_bytes,
                "sha256": a.sha256,
                "runtime": a.runtime.map(|(file, size, sha)| serde_json::json!({ "file": file, "url": LlamaAsset::url(file), "sizeBytes": size, "sha256": sha })),
            })
        })
        .collect();
    serde_json::json!({
        "$comment": "Generated from crates/devall-ai/src/catalog.rs — refresh with UPDATE_LLAMA_ASSETS=1 cargo test -p devall-ai llama_assets_json",
        "build": LLAMA_CPP_BUILD,
        "assets": assets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_sanity() {
        let all = models();
        let mut ids = std::collections::HashSet::new();
        for m in &all {
            assert!(ids.insert(m.id), "duplicate id {}", m.id);
            for field in [m.id, m.name, m.family, m.hf_repo, m.hf_file, m.ollama_tag.unwrap_or("")] {
                let f = field.to_lowercase().replace(['-', '_', ' '], "");
                assert!(!f.contains("qwen2.5") && !f.contains("qwen25"), "Qwen2.5-family model in catalog: {field}");
            }
            assert_eq!(m.license, "Apache-2.0", "{} must be Apache-2.0 for one-click download", m.id);
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(m.hf_file.ends_with(".gguf"));
            assert!(m.size_bytes > 100_000_000);
            assert!(!m.tiers.is_empty());
            assert!(m.download_url().starts_with("https://huggingface.co/"));
        }
        for t in [T8, T16, T24, T32] {
            let r = recommend(t);
            for id in [r.general, r.fim, r.embedding].into_iter().chain(r.alternatives.iter().copied()).chain(r.code) {
                let m = find(id).unwrap_or_else(|| panic!("recommendation {id} not in catalog"));
                assert!(m.tiers.contains(&t) || m.role == ModelRole::Embedding, "{id} recommended for {t:?} but not sized for it");
            }
        }
    }

    #[test]
    fn llama_assets_cover_every_variant() {
        use LlamaVariant::*;
        for v in [MacosArm64, MacosX64, WinCpuX64, WinVulkanX64, WinCuda124X64, WinCuda134X64, WinCpuArm64, WinVulkanArm64, UbuntuX64, UbuntuVulkanX64, UbuntuArm64] {
            let a = llama_asset(v).unwrap();
            assert!(a.file.contains(v.asset_suffix()), "{} vs {}", a.file, v.asset_suffix());
            assert_eq!(a.sha256.len(), 64);
            assert!(LlamaAsset::url(a.file).contains(LLAMA_CPP_BUILD));
        }
        assert!(llama_asset(WinCuda134X64).unwrap().runtime.is_some());
    }

    /// `llama-assets.json` mirrors the pinned assets (the release script verifies downloads with
    /// it); refresh with `UPDATE_LLAMA_ASSETS=1 cargo test -p devall-ai llama_assets_json`.
    #[test]
    fn llama_assets_json_is_up_to_date() {
        let text = format!("{}\n", serde_json::to_string_pretty(&llama_assets_json()).unwrap());
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("llama-assets.json");
        if std::env::var_os("UPDATE_LLAMA_ASSETS").is_some() {
            std::fs::write(&path, &text).unwrap();
        } else {
            let committed = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
            assert_eq!(committed, text, "llama-assets.json is stale — run UPDATE_LLAMA_ASSETS=1 cargo test -p devall-ai llama_assets_json");
        }
    }
}

# Local open-weight LLM update for /dev/all (as of 21 Oct 2026): latest releases, FIM replacement, embeddings, macOS + Windows runtime support

Method note: WebSearch was unavailable for this pass because the shared search budget was exhausted. Every finding below comes from fetching primary pages directly: Hugging Face (HF) model cards, the HF Hub API, GitHub READMEs, releases and issues, and vendor docs.

**HF dates:** dates marked "HF repo created" are the API's `createdAt`. A repo can be created privately before its public release, so treat these as "on or shortly before public release." Where a model card states a release date, that date is used instead.

**Earlier-notes sources:** items marked "(earlier notes)" come from `ai_features_and_local_llm.md` §4 and were not re-fetched on 21 Oct.

**Qwen2.5 rule:** per the user's constraint, no Qwen2.5-family model is recommended anywhere below. Qwen2.5 appears only where a third-party tool still defaults to it, as a warning.

## 1. What is the latest Qwen release as of Oct 2026? Do Qwen3.7 / Qwen3.8 exist?

### Takeaway
**Qwen3.8 exists as open weights, released August 2026. The user is right that Qwen is on 3.8.** But Qwen3.8 has only three open-weight models:
- **Qwen3.8-27B**: dense, Apache-2.0, the only laptop or desktop-GPU-class one.
- **Qwen3.8-Flash-Next**: 125B total / 6B active MoE, plus 51B n-gram embeddings, under a "qwen-community-1.0" licence.
- **Qwen3.8-2.4T-A95B**: under a "qwen3.8-max" licence.

There are **no official Qwen3.8 small models (≤9B) and no Qwen3.8-Coder**.

**Qwen3.7 has no open weights from the Qwen org.** It appears only as API models (Qwen3.7-Plus, Max and Flash) in comparison tables. The newest official small Qwen models remain **Qwen3.5 0.8B/2B/4B/9B** (Feb–Mar 2026). The newest mid-size models are **Qwen3.6-35B-A3B and Qwen3.6-27B** (Apr 2026). The newest Qwen model with documented FIM is **Qwen3-Coder** (30B-A3B, Jul 2025; Next 80B-A3B, Feb 2026).

### Cited Findings

**Qwen org on HF, newest first:**
- Qwen3.8 repos and creation dates:
  - Qwen3.8-27B: 2026-08-05.
  - Qwen3.8-2.4T-A95B and its FP8 version: 2026-08-08.
  - Qwen3.8-27B-FP8: 2026-08-13.
  - Qwen3.8-Flash-Next and its FP8 version: 2026-08-24.
- Before that, the newest LLM repos were:
  - Qwen-AgentWorld-35B-A3B: 2026-06-22.
  - Qwen3.6-35B-A3B: 2026-04-15.
  - Qwen3.6-27B: 2026-04-21.
  - Qwen3.5 small models: 0.8B, 2B, 4B and 9B, with base variants, 2026-02-26 to 02-28.
  - Qwen3.5-27B, 35B-A3B, 122B-A10B and 397B-A17B: 2026-02-16 to 02-24.
  - Qwen3-Coder-Next and its GGUF: 2026-02-01 to 02-02.
- Since Aug 24, the only Qwen-org releases have been non-LLM: Qwen-Image-2.1 (Sept–Oct 9) and Qwen-Drive-1.0-4B (Aug 27).
- **No Qwen3.7 repo exists under the Qwen org.**

  — [HF API: Qwen models by createdAt](https://huggingface.co/api/models?author=Qwen&sort=createdAt&direction=-1&limit=60)
- A Hub-wide search for "Qwen3.7" returns only third-party repos. Two are distillation-target fine-tunes, `nics-efc/Qwen3-8B-ActionOPD-Qwen3.7-Flash` (Sept 29, 2026). The third is a 0-download `RscriptSQwen/Qwen3.7-plus`. There are no official weights. — [HF API: search Qwen3.7](https://huggingface.co/api/models?search=Qwen3.7&sort=downloads&direction=-1&limit=40)
- The Qwen3.8-Flash-Next card lists **Qwen3.7-Plus (397B total / 17B active)** only as a comparison model. The Qwen3.8-2.4T card lists **Qwen3.7-Max** the same way. Neither describes a Qwen3.7 open release. — [HF: Qwen3.8-Flash-Next](https://huggingface.co/Qwen/Qwen3.8-Flash-Next); [HF: Qwen3.8-2.4T-A95B](https://huggingface.co/Qwen/Qwen3.8-2.4T-A95B)

**Qwen3.8-27B**
- Dense, 27B, 64 layers. "Causal Language Model with Vision Encoder" (understands images and video).
- Hybrid attention, "16 × (3 × (Gated DeltaNet → FFN) → 1 × (Gated Attention → FFN))". "Built on the architectural foundation of Qwen3.5."
- Context is "262,144 natively and extensible up to 1,000,000 tokens" via YaRN.
- **Apache-2.0.**
- Thinking is **on by default** and can be disabled per request with `chat_template_kwargs: {enable_thinking: False}`. `reasoning_effort` accepts xhigh (default), medium or low.
- The card's citation says August 2026.
- Self-reported benchmarks: LiveCodeBench v6 90.3, SWE-bench Pro 61.7, Terminal Bench 2.1 73.0, IFBench 79.5, GPQA Diamond 89.2.
- The card says nothing explicit about tool calling, structured output or FIM. It mentions only "broader support for popular harnesses and development tools."
- Runtimes: SGLang, vLLM, TokenSpeed and Transformers. Quantized builds are said to work in "llama.cpp, Ollama, LM Studio". No minimum versions are given.

  — [HF: Qwen/Qwen3.8-27B](https://huggingface.co/Qwen/Qwen3.8-27B)

**Qwen3.8-Flash-Next**
- 125B total / **6B active**, "plus 51B n-gram embedding and 4B MTP". 48 layers mixing Gated DeltaNet, "Qwen Sparse Attention" and MoE, with 512 experts (10 routed + 1 shared active).
- 262K native context, up to 1M.
- **Licence "qwen-community-1.0"**, not Apache. Its terms were not read.
- Vision input. Thinking is on by default.
- Self-reported: LiveCodeBench v6 91.9, SWE-bench Pro 62.5.
- "Qwen3.8-Flash", the version with "1M context length by default" and "official built-in tools", is on Qwen Cloud.

  — [HF: Qwen/Qwen3.8-Flash-Next](https://huggingface.co/Qwen/Qwen3.8-Flash-Next)

**Qwen3.8-2.4T-A95B**
- 2.4T total / 95B active, licence tag "qwen3.8-max". Its API counterpart is Qwen3.8-Max. Thinking is always on.
- Not relevant for local use.

  — [HF: Qwen/Qwen3.8-2.4T-A95B](https://huggingface.co/Qwen/Qwen3.8-2.4T-A95B)

**Ecosystem uptake for Qwen3.8**
- GGUF builds appeared Aug 13–14, 2026:
  - `unsloth/Qwen3.8-27B-GGUF` (Aug 13)
  - `ggml-org/Qwen3.8-27B-GGUF` and `lmstudio-community/Qwen3.8-27B-GGUF` (Aug 14)
  - `bartowski/Qwen3.8-27B-GGUF` (Aug 14)
- MLX builds:
  - `lmstudio-community/Qwen3.8-27B-MLX-4/5/6/8bit` (Aug 14)
  - `mlx-community/Qwen3.8-27B-4bit` (Aug 14)
- `unsloth/Qwen3.8-Flash-Next-GGUF` (Aug 26).
- Qwen3.8-27B had ~7.7M downloads.
- "Qwen3.8-2B/4B/9B-Distill-GGUF" repos (empero-ai, Aug 15) are **third-party distills, not official Qwen models**.

  — [HF API: search Qwen3.8](https://huggingface.co/api/models?search=Qwen3.8&sort=downloads&direction=-1&limit=60)
- **Ollama `qwen3.8`**:
  - Tags `qwen3.8:27b` (18 GB, 256K context, text + image) and `qwen3.8:27b-mlx`.
  - Capability labels: vision, tools, thinking.
  - Description: "Qwen3.8 delivers substantial gains across coding, professional work, research, and long-horizon agentic tasks."

  — [Ollama library: qwen3.8](https://ollama.com/library/qwen3.8)
- **Qwen3.8-27B GGUF sizes (unsloth):**

  | Quant | Size |
  |---|---|
  | UD-IQ3_XXS | 10.93 GB |
  | UD-Q3_K_XL | 13.15 GB |
  | UD-IQ4_XS | 14.25 GB |
  | Q4_0 | 16.06 GB |
  | UD-Q4_K_M | 16.46 GB |
  | UD-Q4_K_XL | 17.56 GB |
  | Q8_0 | 29.05 GB |
  | mmproj (vision) | 0.93 GB |

  — [HF tree: unsloth/Qwen3.8-27B-GGUF](https://huggingface.co/api/models/unsloth/Qwen3.8-27B-GGUF/tree/main)
- The Flash-Next GGUF repo stores its quants in sub-folders (UD-IQ1_M … Q8_0, BF16), so no size is shown. The MTP head alone is 4.14 GB at Q8_0. — [HF tree: unsloth/Qwen3.8-Flash-Next-GGUF](https://huggingface.co/api/models/unsloth/Qwen3.8-Flash-Next-GGUF/tree/main)

**Qwen Coder line**
- The Qwen3-Coder README says "**FIM is supported in every version of Qwen3-Coder**" and documents the format `<|fim_prefix|>…<|fim_suffix|>…<|fim_middle|>`.
- Models listed: Qwen3-Coder-480B-A35B-Instruct, Qwen3-Coder-30B-A3B-Instruct and Qwen3-Coder-Next (with Base), all with 256K context.
- There is no Qwen3.5, 3.6 or 3.8 Coder.

  — [GitHub: QwenLM/Qwen3-Coder](https://github.com/QwenLM/Qwen3-Coder)
- Qwen3-Coder-30B-A3B-Instruct: HF repo created 2025-07-31, Apache-2.0. — [HF API: Qwen3-Coder-30B-A3B-Instruct](https://huggingface.co/api/models/Qwen/Qwen3-Coder-30B-A3B-Instruct)
- **Qwen3-Coder-Next** (collection updated Feb 3, 2026):
  - 80B total / 3B active. Hybrid Gated DeltaNet + Gated Attention + MoE (512 experts, 10 active).
  - 256K context, Apache-2.0.
  - "Supports only non-thinking mode". Tool calling via the `qwen3_coder` parser.

  — [HF: Qwen/Qwen3-Coder-Next](https://huggingface.co/Qwen/Qwen3-Coder-Next)
- Earlier notes: Qwen3.5 and 3.6 "are not FIM families". Qwen3.5-9B has Apache-2.0, thinking on by default, and vendor-reported BFCL-V4 66.1. — [InsiderLLM (earlier notes)](https://insiderllm.com/guides/best-local-coding-models-2026/); [HF: Qwen3.5-9B (earlier notes)](https://huggingface.co/Qwen/Qwen3.5-9B)

### Inferences
- **Answer to the user:** "Qwen 3.8" is correct for the flagship open-weight line (Aug 2026). For local use, though, it means only **Qwen3.8-27B**. Flash-Next is a 125B-total model and is not laptop-class.
- **Small-model tiers:** for 8–16 GB machines, the current official small Qwen models are still Qwen3.5 (0.8B–9B). Recommending "Qwen3.8 4B/9B" would mean using unofficial distills, which is not advised.
- **Licence check:** Qwen3.8 licences differ by size. Only the 27B is Apache-2.0. Flash-Next uses a "qwen-community-1.0" licence, so check its terms before bundling it as a download default.
- **Thinking default:** like Qwen3.5, Qwen3.8 thinks by default. /dev/all should send `enable_thinking: false` (via `chat_template_kwargs`) for interactive and structured-JSON calls.

### Gaps
- I couldn't find an official Qwen blog post giving the exact Qwen3.8 public launch day; the card citations say only "August 2026". Use Aug 13–14, 2026 (when the FP8 weights and the GGUF/MLX ecosystem appeared) as the practical release date.
- The Qwen3.8-27B card doesn't document tool calling. Ollama labels it "tools", but the parser, format and BFCL scores are unverified. Qwen3.8 FIM support is undocumented, so assume no FIM.
- I didn't read the text of the "qwen-community-1.0" and "qwen3.8-max" licences.
- I couldn't fetch r/LocalLLaMA real-world impressions (no search budget).

## 2. Latest releases from other families relevant to small/medium local use

### Takeaway
Since April 2026, the families with new small or medium local-friendly models are:
- **Gemma 4 12B** (May 2026, Apache-2.0)
- **IBM Granite 4.2** (3B/8B/30B dense, Aug 2026, Apache-2.0, official GGUF + MLX)
- **JetBrains Mellum2** (12B-A2.5B code MoE, May–Sept 2026, Apache-2.0)
- **LiquidAI LFM2.5-24B-A2B** (Oct 2026, "lfm1.0" licence, official GGUF/MLX/ONNX)
- **NVIDIA Nemotron 3.5** (Lightning 30B-A3B in Aug 2026; Bolt 74B-A9B "24GB" in Oct 2026; OpenMDW-1.1)

These families have **nothing newer** that is small and local-friendly:
- Llama: still Llama 4, Apr 2025.
- Phi: still Phi-4 family; latest Phi-4-reasoning-vision-15B, Jan 2026.
- SmolLM: still SmolLM3-3B, Jul 2025.
- gpt-oss: still 20b/120b, Aug 2025.
- Mistral: Ministral 3 and Devstral Small 2 (late 2025) are the latest small ones.
- DeepSeek: V4 / V4.1 are large models with no new distills.
- GLM: 5.x, sizes not verified.

### Cited Findings

**Google Gemma**
- **Gemma 4 is still the newest generation.** HF repos:
  - E2B/E4B/26B-A4B/31B "-assistant" variants: Apr 23, 2026.
  - Official **QAT Q4_0 GGUFs**: May 1, 2026.
  - **gemma-4-12B-it**: May 23, 2026.
  - 12B QAT GGUF: Jun 5, 2026.
- Also new: **diffusiongemma-26B-A4B-it** (Jun 9, 2026) and **embeddinggemma-2** (Sept 14, 2026). There is no "Gemma 5".

  — [HF API: google models by createdAt](https://huggingface.co/api/models?author=google&sort=createdAt&direction=-1&limit=50)
- **Gemma 4 12B card:**
  - **Apache-2.0**.
  - Dense, 11.95B. "Encoder-free" "Unified" architecture.
  - 256K context. Text, image and audio input.
  - "Native support for structured tool use". Thinking is toggled via the `<|think|>` token or `enable_thinking`.
  - Family sizes:

    | Model | Parameters | Context |
    |---|---|---|
    | E2B | 2.3B effective (5.1B with embeddings) | 128K |
    | E4B | 4.5B effective (8B with embeddings) | 128K |
    | 12B | 11.95B dense | 256K |
    | 26B A4B | 25.2B total / 3.8B active | 256K |
    | 31B | 30.7B dense | 256K |

  - 12B scores: LiveCodeBench v6 72.0, Tau2 69.0, MMLU-Pro 77.2.

  — [HF: google/gemma-4-12B-it](https://huggingface.co/google/gemma-4-12B-it)
- Gemma 4 QAT Q4_0 weight memory: E2B 2.9 GB, E4B 4.5 GB, 12B 6.7 GB, 26B-A4B 14.4 GB, 31B 17.5 GB (weights only). Gemma 4 was released Apr 2, 2026. — [Google AI: Gemma core docs (earlier notes)](https://ai.google.dev/gemma/docs/core)

**IBM Granite 4.2**
- Repos for granite-4.2-3b, 8b and 30b were created Aug 7, 2026. Official **GGUF** came Aug 12, FP8/MXFP4/NVFP4 Aug 13, and official **MLX** (q4/q8/bf16) Sept 1.
- Also new: granite-switch-4.1-8b/30b (Oct 14, 2026), granite-vision-4.1-4b-GGUF (Jun 2026), granite-swash-2b and 3b-a600m (Jul 1, 2026).

  — [HF API: ibm-granite by createdAt](https://huggingface.co/api/models?author=ibm-granite&sort=createdAt&direction=-1&limit=40)
- **Granite-4.2-8B card:**
  - Release date "August 25, 2026". The card's date conflicts with the Aug 7 repo creation.
  - "Decoder-only Dense Transformer (Reasoning)". **3B, 8B and 30B are all dense.**
  - Context "Natively Supports 128K (Long-context extension to 512K)".
  - **Apache-2.0**. 12 languages.
  - "supports tool calling with integrated reasoning" (OpenAI function schema).
  - Thinking is on by default. `enable_thinking=False` turns it off; a `low_effort=True` mode also exists.
  - 8B scores: BFCL v4 52.39, τ³-bench 58.06, SWE-bench Verified 47.67, LiveCodeBench v6 73.24, MMLU-Pro 74.04, IFBench 79.33.
  - vLLM v0.20+ (custom `granite_thinking_parser`) and SGLang v0.5.18+. "llama.cpp, Ollama, LM Studio" for quants.
  - **FIM is not mentioned** in the 4.2 card.

  — [HF: ibm-granite/granite-4.2-8b](https://huggingface.co/ibm-granite/granite-4.2-8b)
- Granite-4.2-8B official GGUF sizes: Q4_K_M 5.35 GB, Q4_0 5.06 GB, Q5_K_M 6.25 GB, Q8_0 9.35 GB. — [HF tree: granite-4.2-8b-GGUF](https://huggingface.co/api/models/ibm-granite/granite-4.2-8b-GGUF/tree/main)
- Earlier notes: Granite **4.0** documents FIM, JSON output and tool calling, with Nano sizes of 1B and 350M. — [IBM Granite docs (earlier notes)](https://www.ibm.com/granite/docs/use-cases/prompt-engineering)

**Mistral**
- No new small general LLM since late 2025. Recent repos:
  - Timestral time-series models (Oct 14, 2026)
  - Voxtral-Mini-4B-Realtime-Arabic (Oct 8)
  - Shieldstral-1.0-3B, a safety model (Jul 16)
  - Leanstral-1.5-119B-A6B (Jul 1)
  - Mistral-Medium-3.5-128B (Mar 31, 2026)
  - Mistral-Small-4-119B-2603 (repo Jan 23, 2026; now 119B, not "small")
  - Devstral-Small-2-24B-Instruct-2512 and Devstral-2-123B (Nov 28, 2025)
  - **Ministral-3 3B/8B/14B**, Instruct and Reasoning, with official GGUF (repos Oct 31, 2025; "2512" naming)

  — [HF API: mistralai by createdAt](https://huggingface.co/api/models?author=mistralai&sort=createdAt&direction=-1&limit=40)
- Ollama lists "**mistral-large-4**" as an "open-weight" MoE (1.05T total / 52B active, 1M context), but only as a cloud model. No such repo appears in Mistral's HF org, so its open-weight status is unverified. — [Ollama search (newest)](https://ollama.com/search?o=newest)

**Meta Llama**
- The newest meta-llama repos are still **Llama 4 Scout/Maverick (Apr 2025)**, plus Llama-Guard-4 and Prompt-Guard-2 (Apr 2025). There is no 2026 release. — [HF API: meta-llama by createdAt](https://huggingface.co/api/models?author=meta-llama&sort=createdAt&direction=-1&limit=25)

**Microsoft Phi**
- Newest: Phi-Ground-Any (May 7, 2026), **Phi-4-reasoning-vision-15B** (Jan 23, 2026) and paza-Phi-4-multimodal (Jan 2026).
- The small text models are still **Phi-4-mini-instruct (Feb 2025)** and Phi-4-mini-flash-reasoning (Jun 2025). There is no Phi-5.

  — [HF API: microsoft phi](https://huggingface.co/api/models?author=microsoft&search=phi&sort=createdAt&direction=-1&limit=30)

**DeepSeek**
- DeepSeek-V4-Flash and V4-Pro (with Base) were released Apr 22, 2026. Updates followed:
  - V4-Flash-0731
  - V4-Pro-0813
  - V4-Flash-Vision-Exp (Aug 31)
  - **V4.1-Flash** (Sept 10)
  - **V4.1-Flash-1017** (Oct 17, 2026)
- I found **no R2 and no new small distills**. DeepSeek did publish speculative-decoding draft models (eagle3, dflash and dspark) for **gemma4-12b** and **Qwen3 4B/8B/14B** (Jun 28, 2026).

  — [HF API: deepseek-ai by createdAt](https://huggingface.co/api/models?author=deepseek-ai&sort=createdAt&direction=-1&limit=30)
- Ollama lists deepseek-v4.1-flash as a **cloud** model (1M context). — [Ollama search (newest)](https://ollama.com/search?o=newest)

**OpenAI gpt-oss**
- Still **gpt-oss-20b and gpt-oss-120b (Aug 4–5, 2025)** and gpt-oss-safeguard-20b/120b (Sept 2025). Since then OpenAI has added only privacy-filter (Apr 2026) and circuit-sparsity (Dec 2025). There is no gpt-oss-2. — [HF API: openai by createdAt](https://huggingface.co/api/models?author=openai&sort=createdAt&direction=-1&limit=25)
- llama.vscode says gpt-oss 20B is "the best choice for now" as a local agent model. — [GitHub: ggml-org/llama.vscode](https://github.com/ggml-org/llama.vscode)

**NVIDIA Nemotron 3.5**
- Releases:
  - NVIDIA-Nemotron-3.5-Lightning-30B-A3B (BF16/NVFP4, Aug 1–5, 2026)
  - Nemotron-3.5-Embed-1B and 8B (Oct 7)
  - NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB, with an official **GGUF** (Oct 7–12)
  - Nemotron-3.5-Super-VL-120B-A12B (Oct 12)
  - Nemotron-3.5-Rerank-8B (Oct 15)
  - NVIDIA-Nemotron-Labs-Dory1.0-4B, with GGUF (Oct 16; purpose not checked)

  — [HF API: nvidia Nemotron](https://huggingface.co/api/models?author=nvidia&search=Nemotron&sort=createdAt&direction=-1&limit=40)
- **Nemotron 3.5 Bolt 74B-A9B-24GB:**
  - "Mamba-2–Transformer hybrid LatentMoE" with a vision encoder.
  - Uses "a native 2-bit quantization to fit in 24GB of memory": experts at IQ2-XXS, Mamba layers at NVFP4.
  - 256K context. **OpenMDW-1.1**, "ready for commercial or non‑commercial use".
  - Tool calling (qwen3_coder parser). Reasoning toggled with `enable_thinking`.
  - The card lists vLLM nightly as the runtime. Its hardware list says Blackwell SM120/121, which is inconsistent with the Ampere/Hopper mention elsewhere on the card.

  — [HF: Nemotron-3.5-Bolt-74B-A9B-24GB](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB)
- Ollama v0.40.5 (Oct 14) added Nemotron 3.5 Super and Bolt (Bolt "tuned to fit a 24 GB NVIDIA GPU"), with "updated llama.cpp with Nemotron 3.5 support". — [Ollama releases](https://github.com/ollama/ollama/releases)

**LiquidAI LFM2.5**
- **LFM2.5-24B-A2B** (repos Oct 9–12, 2026):
  - 24B total / 2.45B active. 30 short-conv + 10 GQA layers, 64 experts top-4.
  - 128K context, 16 languages.
  - "a pure reasoning model that always thinks before it answers".
  - Tool calling (Pythonic by default; JSON via system prompt).
  - Licence tag "**lfm1.0**" (terms not stated on the card).
  - Official **GGUF, MLX (4–8 bit, bf16) and ONNX**.
  - Vendor-reported speed: 184 tok/s on an M5 Max.

  — [HF: LiquidAI/LFM2.5-24B-A2B](https://huggingface.co/LiquidAI/LFM2.5-24B-A2B)
- Smaller LFM2.5 models exist: 1.2B-Instruct, 2.6B, 8B-A1B and VL-3B (GGUF/DSpark repos Aug 2026). LiquidAI also released "d1" decision models (3B and 600M omni, Oct 2026). — [HF API: LiquidAI by createdAt](https://huggingface.co/api/models?author=LiquidAI&sort=createdAt&direction=-1&limit=30)

**JetBrains Mellum2**
- Releases:
  - Mellum2-12B-A2.5B Base/Instruct/Thinking (May 26, 2026)
  - Official GGUF quants Q4_K_M/Q6_K/Q8_0/MXFP4_MOE/BF16 (Jun 3–4)
  - **Mellum2.1-12B-A2.5B-Thinking** (Sept 20) and its GGUF (Oct 7, 2026)
  - "Qwen3.8-3.6-27B-blend" (GGUF + MLX, Sept 15)

  — [HF API: JetBrains by createdAt](https://huggingface.co/api/models?author=JetBrains&sort=createdAt&direction=-1&limit=30)
- See §3 for card details.

**HF SmolLM**
- Newest: SmolLM3-3B (Jul 8, 2025) and nanowhale-100m (Apr 2026). There is no SmolLM4. — [HF API: HuggingFaceTB](https://huggingface.co/api/models?author=HuggingFaceTB&sort=createdAt&direction=-1&limit=20)

**Zhipu GLM**
- GLM-5.3 and GLM-5.3-Flash (Aug 25, 2026), GLM-5.2 (Jun 16), GLM-5.1 (Apr 3), GLM-5 (Feb 11) and GLM-4.7-Flash (Jan 19, 2026). — [HF API: zai-org by createdAt](https://huggingface.co/api/models?author=zai-org&sort=createdAt&direction=-1&limit=25)

**Other newest Ollama entries (Oct 2026)**
- Local:
  - **bonsai-2**, ternary-weight vision models, 2b/4b/9b/27b, tools + thinking
  - **embeddinggemma-2** (270m/440m/570m/740m)
  - **nemotron-3.5-bolt** ("compact vision-language model for local coding, reasoning, and tool use")
  - "**decision models**" (nimble 9B, tev1 0.8b/4b, clef-flash 9B, d1, pplx-decider 27B "fine-tuned from Qwen3.8-27B")
- Cloud-only: minimax-m3.1-flash, beam (Reflection), muse-spark-open (816B), mistral-large-4, deepseek-v4.1-flash.

  — [Ollama search (newest)](https://ollama.com/search?o=newest)
- Ollama introduced decision models through `/v1/systemone` in v0.35.0 (Sept 28, 2026). They return "choices, probabilities, and scores instead of text". This confirms what the earlier notes listed as unverified. — [Ollama releases](https://github.com/ollama/ollama/releases)

### Inferences
- **Best small and medium general choices for /dev/all in Oct 2026:**
  - **Qwen3.5-4B/9B**
  - **Gemma 4 (E4B / 12B / 26B-A4B)**
  - **Granite 4.2 8B**: the most "enterprise-clean" pick (Apache-2.0, official GGUF + MLX from the vendor, dense and predictable, tool calling).
  - **Qwen3.8-27B** at 24–32 GB+.
- **Licence hygiene:**
  - Prefer Apache-2.0 models (Qwen3.5, Qwen3.8-27B, Gemma 4, Granite 4.2, Mellum, Qwen3-Coder).
  - Flag "lfm1.0", "qwen-community-1.0" and OpenMDW-1.1 for review before making them one-click defaults.
- **Ollama decision models:** these are a new primitive that may suit /dev/all's classify, route and score tasks (e.g. "is this response an error?"). They are Ollama-specific (`/v1/systemone`), though, not OpenAI-compatible.

### Gaps
- Licence text not verified for Ministral 3, Devstral Small 2, LFM2.5 ("lfm1.0") and GLM-5.3.
- Parameter counts not verified for GLM-5.3 and GLM-5.3-Flash, DeepSeek V4/V4.1-Flash and Nemotron 3.5 Lightning.
- Nemotron-Labs-Dory1.0-4B's purpose was not checked.
- Mistral Large 4's open-weight status is unverified (it appears on Ollama but not in Mistral's HF org).
- Granite 4.2 3B GGUF size and FIM support for Granite 4.2 were not verified.

## 3. Best current replacement for Qwen2.5-Coder for low-latency inline autocomplete (FIM)

### Takeaway
- **For machines with ≥24 GB VRAM or ≥32 GB unified memory:** Qwen3-Coder-30B-A3B-Instruct is the replacement.
  - Released Jul 2025, Apache-2.0, FIM officially supported, 3B active so it is fast.
  - Q4_K_M is 18.6 GB; IQ4_XS is 16.4 GB; Q3_K_M is 14.7 GB.
  - llama-server already has a `--fim-qwen-30b-default` preset for it.
  - Qwen3-Coder-Next (80B-A3B, Feb 2026) is the step up for 64 GB+ machines.
- **For 8–16 GB machines:** there is **no verified 2026 small FIM model**. The best verified non-Qwen2.5 option is **JetBrains Mellum-4b** (sft-all / dpo-all, 2025, Apache-2.0, explicit FIM tokens, 8K context, built for IDE completion).
- **Avoid tools' Qwen2.5 defaults:** llama.cpp's and llama.vscode's small FIM presets (`--fim-qwen-1.5b/3b/7b-default`, `-7b-spec`, `-14b-spec`) all still load **Qwen 2.5 Coder**. /dev/all must not use those presets.

### Cited Findings
- **llama-server built-in presets:**
  - `--fim-qwen-1.5b-default`: "use default Qwen 2.5 Coder 1.5B"
  - `--fim-qwen-3b-default` and `--fim-qwen-7b-default`: Qwen 2.5 Coder 3B / 7B
  - `--fim-qwen-7b-spec` and `--fim-qwen-14b-spec`: Qwen 2.5 Coder with a 0.5B draft model
  - `--fim-qwen-30b-default`: "use default **Qwen 3 Coder 30B A3B Instruct**"
  - Also `--gpt-oss-20b-default`, `--embd-gemma-default`, and `--vision-gemma-4b/12b-default` (Gemma 3 QAT)
  - The `/infill` endpoint "Takes a prefix and a suffix and returns the predicted completion as stream". It supports repo-level FIM when the model has `FIM_REPO`/`FIM_FILE_SEP` tokens.

  — [llama.cpp server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- **llama.vscode FIM tiers:**

  | Tier | Command |
  |---|---|
  | >64 GB | `llama serve --fim-qwen-30b-default` |
  | >16 GB | `--fim-qwen-7b-default` |
  | <16 GB | `--fim-qwen-3b-default` |
  | <8 GB | `--fim-qwen-1.5b-default` |
  | CPU-only | `ggml-org/Qwen2.5-Coder-1.5B-Q8_0-GGUF` / `0.5B` |

  Only the 30B tier uses a model newer than Qwen2.5. The extension auto-installs llama.cpp on Mac (Homebrew) and Windows (winget).

  — [GitHub: ggml-org/llama.vscode](https://github.com/ggml-org/llama.vscode)
- Qwen3-Coder README: "FIM is supported in every version of Qwen3-Coder", using the format `<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>`. — [GitHub: QwenLM/Qwen3-Coder](https://github.com/QwenLM/Qwen3-Coder)
- Qwen3-Coder-30B-A3B-Instruct: HF created 2025-07-31, Apache-2.0. — [HF API](https://huggingface.co/api/models/Qwen/Qwen3-Coder-30B-A3B-Instruct)
- Qwen3-Coder-30B-A3B GGUF sizes (unsloth):

  | Quant | Size |
  |---|---|
  | UD-Q2_K_XL | 11.79 GB |
  | UD-IQ3_XXS | 12.85 GB |
  | Q3_K_M | 14.71 GB |
  | IQ4_XS | 16.38 GB |
  | UD-Q4_K_XL | 17.67 GB |
  | Q4_K_M | 18.56 GB |
  | Q8_0 | 32.48 GB |

  — [HF tree: unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF](https://huggingface.co/api/models/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/tree/main)
- **Qwen3-Coder-Next:** 80B / 3B active, Apache-2.0, 256K context, non-thinking only. Earlier notes put Q4 at ≈38 GB. — [HF: Qwen3-Coder-Next](https://huggingface.co/Qwen/Qwen3-Coder-Next); [InsiderLLM (earlier notes)](https://insiderllm.com/guides/best-local-coding-models-2026/)
- **JetBrains Mellum-4b-dpo-all:**
  - "LLaMA-style architecture with 4 billion parameters", "context window of 8192 tokens", **Apache-2.0**.
  - FIM tokens `<filename>`, `<fim_suffix>`, `<fim_prefix>`, `<fim_middle>`; the suffix goes before the prefix.
  - "tailored for context-aware code completion tasks", "Designed for integration into professional developer tooling (e.g., intelligent code suggestions in IDEs)".
  - About 40 languages, including Rust, TS, Go, SQL and Shell.
  - Quantized variants exist for llama.cpp, Ollama and LM Studio.
  - JetBrains' own GGUF repos: Mellum-4b-sft-all-gguf (Jul 17, 2025) and Mellum-4b-dpo-all-gguf (Sept 30, 2025).

  — [HF: JetBrains/Mellum-4b-dpo-all](https://huggingface.co/JetBrains/Mellum-4b-dpo-all); [HF API: JetBrains](https://huggingface.co/api/models?author=JetBrains&sort=createdAt&direction=-1&limit=30)
- **JetBrains Mellum2-12B-A2.5B-Base** (tech report May 29, 2026):
  - MoE, 64 experts with 8 active (~2.5B active). 131K context with a 1,024 sliding window. **Apache-2.0**.
  - The card **does not mention FIM**. It positions Base as a fine-tuning starting point ("use the completions endpoint, not chat").
  - Code scores are modest: HumanEval 41.5, MBPP 62.4. The card says Base "trails Qwen2.5 7B and Qwen3 4B" on most code benchmarks.
  - Instruct Q4_K_M GGUF: **8.07 GB**.

  — [HF: Mellum2-12B-A2.5B-Base](https://huggingface.co/JetBrains/Mellum2-12B-A2.5B-Base); [HF tree: Mellum2 Instruct Q4_K_M](https://huggingface.co/api/models/JetBrains/Mellum2-12B-A2.5B-Instruct-GGUF-Q4_K_M/tree/main)
- **FIM is not documented** on the cards for Qwen3.8-27B, Qwen3.8-Flash-Next, Granite 4.2 8B, Gemma 4 12B or LFM2.5-24B-A2B. — [HF: Qwen3.8-27B](https://huggingface.co/Qwen/Qwen3.8-27B); [HF: granite-4.2-8b](https://huggingface.co/ibm-granite/granite-4.2-8b); [HF: gemma-4-12B-it](https://huggingface.co/google/gemma-4-12B-it); [HF: LFM2.5-24B-A2B](https://huggingface.co/LiquidAI/LFM2.5-24B-A2B)
- Earlier notes say IBM documents FIM for Granite **4.0**, but not specifically for 4.2. — [IBM Granite docs (earlier notes)](https://www.ibm.com/granite/docs/use-cases/prompt-engineering)

### Inferences
- **Recommended FIM ladder for /dev/all, with no Qwen2.5:**
  - **≥24 GB NVIDIA, or ≥32 GB Apple Silicon:** Qwen3-Coder-30B-A3B-Instruct, Q4_K_M (18.6 GB) or IQ4_XS (16.4 GB). Use it through llama-server `/infill`, or with the preset `--fim-qwen-30b-default`.
    - The preset name contains "qwen" but loads Qwen3-Coder, which is allowed.
    - 3B active parameters give low per-token latency.
  - **16 GB machines:** Qwen3-Coder-30B-A3B at Q3_K_M (14.7 GB) is too tight on a 16 GB Mac, which also needs RAM for the OS and IDE. Use **Mellum-4b** instead.
  - **8 GB machines:** Mellum-4b at Q4/Q8, if RAM allows; otherwise disable FIM.
- **Fallback when no FIM model is installed:** /dev/all can do "prefix-only" completion with the general model. In its script and request editors, a short-prompt chat completion with tight `max_tokens` is often enough. Mark this as lower quality.
- **Watch item:** JetBrains Mellum2 or Mellum2.1 might gain FIM. Re-check its card and the llama.vscode/llama.cpp presets each quarter, since upstream will likely add newer FIM presets.
- **/dev/all's own llama-server sidecar** should define its own FIM model choices and not rely on upstream `--fim-qwen-*-default` presets, which still point to Qwen2.5 for small sizes.

### Gaps
- I found no 2026-released small (≤8B) model with documented FIM.
- No latency or quality comparison was found for Mellum-4b vs Qwen3-Coder-30B-A3B FIM, and there are no SAFIM/RepoBench numbers on the cards.
- Mellum-4b GGUF file sizes were not fetched. A 4B model is roughly 2.5 GB at Q4 by rule of thumb; this is unverified.

## 4. Current best small embedding models and rerankers for local semantic search

### Takeaway
- **Default embedder: EmbeddingGemma 2** (Google, HF repo Sept 14, 2026, **Apache-2.0**).
  - Loads as a 270M text-only model, or up to 740M for full multimodal.
  - 768-d with Matryoshka 512/256/128. 8K context.
  - MTEB Code v1 rises to 78.68, from 68.76 for v1.
  - Already in Ollama (with MLX on Apple Silicon).
- **Alternatives:**
  - **Qwen3-Embedding-0.6B** (Jun 2025, Apache-2.0, official GGUF). This is a Qwen3 model, not Qwen2.5, so it is acceptable.
  - **Nemotron-3.5-Embed-1B** (Oct 2026, OpenMDW-1.1, 32K context, multimodal). Heavier, with no GGUF/ONNX mentioned.
- **Rerankers:**
  - Qwen3-Reranker-0.6B/4B/8B (May–Jun 2025)
  - Qwen3-VL-Reranker-2B/8B (Jan 2026)
  - Nemotron-3.5-Rerank-8B (Oct 15, 2026)
  - For small local use, **Qwen3-Reranker-0.6B** is the practical pick.

### Cited Findings
- **EmbeddingGemma 2:**
  - "740M total parameters, combining a 270M parameter text model" ("130M transformer + 140M embedder"). Load configurations: 270M text, 440M text + image, 570M text + audio, 740M full.
  - Native 768-d with truncation to 512/256/128 ("Re-normalize after truncating").
  - 8,192-token context shared across modalities. Text including code, images, video and audio in one space.
  - **Apache-2.0**; the licence link points to the Gemma 4 licence page, and no gating is mentioned.
  - MTEB multilingual v2: 61.36. **MTEB Code v1: 78.68, vs 68.76 for EmbeddingGemma 1.**
  - "Do not use float16." Sentence-Transformers is the primary runtime.
  - The card doesn't mention ONNX or GGUF, though there are 109 quantized derivatives.

  — [HF: google/embeddinggemma-2](https://huggingface.co/google/embeddinggemma-2); repo date from the [HF API](https://huggingface.co/api/models?author=google&sort=createdAt&direction=-1&limit=50)
- Ollama lists `embeddinggemma-2` with tags 270m, 440m, 570m and 740m (vision, embedding, audio), ~72K downloads. — [Ollama search](https://ollama.com/search?o=newest)
- Ollama v0.40.0 (Sept 25, 2026) runs embeddinggemma-2 on MLX on Apple Silicon. v0.40.4 added "multimodal embeddings through llama-server". — [Ollama releases](https://github.com/ollama/ollama/releases)
- llama-server has an `--embd-gemma-default` preset ("use default EmbeddingGemma model"). The README doesn't say whether it means v1 or v2. — [llama.cpp server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- **Qwen embeddings and rerankers on HF:**
  - Qwen3-Embedding-0.6B, 4B and 8B (Jun 3, 2025), with official GGUFs (Jun 5, 2025).
  - Qwen3-VL-Embedding-2B and 8B (Jan 7, 2026).
  - Qwen3-Reranker-0.6B and 8B (May 29, 2025), 4B (Jun 3, 2025).
  - Qwen3-VL-Reranker-2B and 8B (Jan 7, 2026).

  — [HF API: Qwen Embedding](https://huggingface.co/api/models?author=Qwen&search=Embedding&sort=createdAt&direction=-1&limit=20); [HF API: Qwen Reranker](https://huggingface.co/api/models?author=Qwen&search=Reranker&sort=createdAt&direction=-1&limit=20)
- Qwen3-Embedding-0.6B: MTEB-eng-v2 70.7, Apache-2.0, ≈1.5 GB (BF16). — [D-Central (earlier notes)](https://d-central.tech/local-embedding-models/)
- **Nemotron-3.5-Embed-1B-BF16** (the card says "Hugging Face 10/20/2026"; repo Oct 7):
  - 1.55B parameters. 2048-d, truncatable to 1024/512. 32,768-token context.
  - 34 languages for text. Image retrieval too.
  - **OpenMDW-1.1**, "ready for commercial or non-commercial use".
  - Runtimes: Transformers, Sentence-Transformers and vLLM. No ONNX, GGUF or Ollama.
  - Reports ViDoRe/MIRACL-VISION scores, not MTEB.

  — [HF: Nemotron-3.5-Embed-1B-BF16](https://huggingface.co/nvidia/Nemotron-3.5-Embed-1B-BF16)
- Nemotron-3.5-Rerank-8B was created on HF Oct 15, 2026. — [HF API: nvidia Nemotron](https://huggingface.co/api/models?author=nvidia&search=Nemotron&sort=createdAt&direction=-1&limit=40)
- Community rerankers in active use (Oct 2026) include `gte-reranker-modernbert-base` (community GGUFs exist) and bge-reranker-v2-m3 fine-tunes. — [HF API: text-ranking models](https://huggingface.co/api/models?search=Reranker&sort=createdAt&direction=-1&limit=40&filter=text-ranking)
- Earlier-notes options remain valid: BGE-M3 (MIT, 8K context, 100+ languages), nomic-embed-text (≈0.3 GB, simple via Ollama) and all-MiniLM-L6-v2 (≈0.1 GB, CPU-only). Avoid NV-Embed-v2 and jina-embeddings-v3, which are non-commercial. — [D-Central (earlier notes)](https://d-central.tech/local-embedding-models/); [bestllmfor.com (earlier notes)](https://bestllmfor.com/best/embeddings-self-hosted/)

### Inferences
- **For /dev/all's in-process embedder** (ort or candle, Tier C), there are two paths:
  - **EmbeddingGemma 2, 270M text-only:** best code-retrieval score and Apache-2.0, which removes the earlier gated-licence friction of EmbeddingGemma 1. But no official ONNX export was verified.
  - **Qwen3-Embedding-0.6B:** official GGUF, so it works through a llama-server `--embedding` sidecar or Ollama.
  - **Suggested approach:** default to EmbeddingGemma 2 via Ollama/llama-server when available. Ship Qwen3-Embedding-0.6B or EmbeddingGemma ONNX in-process once an ONNX export is verified.
- **Index stability:** store the embedding model ID and dimension with each index. Matryoshka truncation (EmbeddingGemma 2: 256-d) cuts storage for large API-collection or history indexes.
- **Rerankers:** for a small local reranker, use Qwen3-Reranker-0.6B. Nemotron-3.5-Rerank-8B is too heavy for default use.

### Gaps
- No verified ONNX export for EmbeddingGemma 2, and none for Nemotron-3.5-Embed.
- Licences for the Qwen3-Reranker and Qwen3-VL-Embedding/Reranker models were not re-verified this pass (the Qwen3-Embedding line is Apache-2.0 per earlier notes).
- No independent MTEB comparison of EmbeddingGemma 2 vs Qwen3-Embedding-0.6B on the same harness. MTEB variants differ (EmbeddingGemma 2 reports multilingual v2 and Code v1; Qwen reports eng-v2).
- FastEmbed-rs support for EmbeddingGemma 2 or Qwen3-Embedding was not checked.

## 5. Availability: GGUF (llama.cpp / Ollama / LM Studio) and MLX; minimum runtime versions

### Takeaway
Every recommended model has **GGUF** builds, most from the vendor or from ggml-org, unsloth or lmstudio-community. Most also have **MLX** builds:
- Qwen3.8-27B: lmstudio-community and mlx-community.
- Granite 4.2: official IBM MLX.
- LFM2.5: official MLX.
- Gemma 4, Qwen3.5/3.6/3.8 and EmbeddingGemma-2: Ollama runs them on MLX automatically since v0.40.0.

**Version floors:**
- **Ollama ≥ v0.40.x** is needed for the 2026 architectures. v0.40.5+ is needed for Nemotron 3.5. Models upgraded by v0.40.2 must be re-pulled if you downgrade below 0.40.
- **llama.cpp:** pin a recent build (b11796 is current, Oct 21, 2026). Per-architecture minimum build numbers could not be verified.

### Cited Findings
- **Qwen3.8-27B formats:**
  - GGUF from unsloth, ggml-org, lmstudio-community and bartowski (Aug 13–14, 2026).
  - MLX: lmstudio-community 4/5/6/8-bit and mlx-community 4-bit (Aug 14).
  - NVFP4 from nvidia, unsloth and RedHat (INT4); AWQ.

  — [HF API: search Qwen3.8](https://huggingface.co/api/models?search=Qwen3.8&sort=downloads&direction=-1&limit=60)
- Ollama has `qwen3.8:27b` and `qwen3.8:27b-mlx` (both 18 GB). — [Ollama: qwen3.8](https://ollama.com/library/qwen3.8)
- **Gemma 4:** official Google QAT Q4_0 GGUF for E2B/E4B/26B-A4B/31B (May 1, 2026) and 12B (Jun 5, 2026). — [HF API: google](https://huggingface.co/api/models?author=google&sort=createdAt&direction=-1&limit=50)
- **Granite 4.2:** official GGUF (Aug 12, 2026) and official MLX q4/q8/bf16 for 3B/8B/30B (Sept 1, 2026). — [HF API: ibm-granite](https://huggingface.co/api/models?author=ibm-granite&sort=createdAt&direction=-1&limit=40)
- **LFM2.5-24B-A2B:** official GGUF, ONNX and MLX (4/5/6/8-bit, bf16). — [HF API: LiquidAI](https://huggingface.co/api/models?author=LiquidAI&sort=createdAt&direction=-1&limit=30)
- **Mellum2:** official GGUF (Q4_K_M/Q6_K/Q8_0/MXFP4_MOE/BF16). **Qwen3-Coder-Next:** official GGUF. **Qwen3-Embedding:** official GGUF. — [HF API: JetBrains](https://huggingface.co/api/models?author=JetBrains&sort=createdAt&direction=-1&limit=30); [HF API: Qwen](https://huggingface.co/api/models?author=Qwen&sort=createdAt&direction=-1&limit=60)
- **Ollama releases:**
  - v0.40.0 (Sept 25, 2026): "model architectures supported by the MLX runtime will automatically run on MLX" on Apple Silicon, including gemma4, qwen3.6, qwen3.5, qwen3.8 (a pull example) and embeddinggemma-2.
  - v0.40.2 (Oct 8): background upgrades of older models "for llama.cpp compatibility", and "If you later downgrade to a version older than 0.40, you'll need to re-pull those models."
  - v0.40.5 (Oct 14): Nemotron 3.5 support.
  - v0.40.7 (Oct 18): the latest release.

  — [Ollama releases](https://github.com/ollama/ollama/releases)
- **llama.cpp b11796** (published 2026-10-21T12:34Z) is the latest build. Its notes cover only an mtmd change and name no newly supported architectures. — [llama.cpp releases](https://github.com/ggml-org/llama.cpp/releases); [GitHub API: latest release](https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=1)
- Model cards give no minimum llama.cpp or Ollama versions. Qwen3.8 says "latest framework versions". Granite gives vLLM ≥0.20 and SGLang ≥0.5.18. Nemotron Bolt needs vLLM nightly. — [HF: Qwen3.8-Flash-Next](https://huggingface.co/Qwen/Qwen3.8-Flash-Next); [HF: granite-4.2-8b](https://huggingface.co/ibm-granite/granite-4.2-8b); [HF: Nemotron Bolt](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB)

### Inferences
- **Sidecar version policy:**
  - Pin a specific llama.cpp build per /dev/all release, e.g. ≥b11796 for the Oct 2026 lineup. Each curated model should carry a `min_llamacpp_build` field.
  - For Ollama, read `/api/version` and warn below 0.40.0 for Qwen3.8, Gemma 4 or EmbeddingGemma-2, and below 0.40.5 for Nemotron 3.5.
  - Ollama's MLX path on Macs means /dev/all does not need its own MLX engine. Connecting to Ollama gives Mac users MLX speed for free.
- **Tool calling on Qwen3.8 / Qwen3.5 / Granite / Nemotron:** these use the `qwen3_coder` tool parser family in vLLM. In llama.cpp, tool calling depends on the Jinja chat template (`--jinja` is on by default). Validate each curated model with a tool-call smoke test in CI.

### Gaps
- No per-architecture "first supported llama.cpp build" was found for Qwen3.8 (Qwen3.5-style hybrid with Gated DeltaNet), Granite 4.2, Mellum2, LFM2.5-24B-A2B, Nemotron 3.5 or EmbeddingGemma-2. The release notes don't name them, and search was unavailable.
- Mellum2 and Gemma 4 MLX repos from mlx-community were not directly checked. Ollama's MLX runner covers gemma4.
- LM Studio's per-model availability (staff picks) was not checked.

## 6. macOS + Windows runtime support (llama.cpp, Ollama, LM Studio, ort, candle) and platform pitfalls

### Takeaway
- **llama.cpp sidecar:** prebuilt for both OSes. The current build b11796 has:
  - macOS arm64 (16 MB) and **macOS x64 / Intel (12 MB)**.
  - Windows CPU x64 (24 MB) and CPU ARM64 (13 MB).
  - **Vulkan x64 (37 MB) and Vulkan ARM64 (25 MB)**, covering AMD, Intel and generic GPUs.
  - CUDA 13.4 x64 (160 MB **+ 424 MB cudart**) and CUDA 12.4 x64 (272 MB + 391 MB cudart).
  - CUDA 13.4 ARM64, OpenCL-Adreno ARM64 (Snapdragon), SYCL (Intel), ROCm 10.0 and OpenVINO.
- **Ollama:** both OSes. macOS 14+, with Intel Macs CPU-only. Windows 10 22H2+, with NVIDIA, or AMD via ROCm v7 / Vulkan.
- **LM Studio:** **Apple Silicon only on macOS** (no Intel Macs), macOS 14+. Windows x64 (AVX2) and ARM64 (Snapdragon).
- **ort:** CoreML on macOS. On Windows, DirectML (now "sustained engineering"; ONNX Runtime has moved new work to WinML) or CUDA.
- **Pitfalls:**
  - Windows Defender and Bitdefender **false positives on llama.cpp Windows binaries are recurring** (six issues, Jul–Sep 2026).
  - The CUDA runtime adds ~0.4 GB.
  - macOS needs Developer ID signing and notarization.

### Cited Findings
- **llama.cpp b11796 assets** (2026-10-21; sizes in MB from the GitHub API):

  | Asset | Size (MB) |
  |---|---|
  | macos-arm64 | 16.14 |
  | macos-x64 | 11.87 |
  | xcframework | 66.79 |
  | win-cpu-x64 | 24.04 |
  | win-cpu-arm64 | 12.68 |
  | win-vulkan-x64 | 36.84 |
  | win-vulkan-arm64 | 25.07 |
  | win-cuda-13.4-x64 | 159.70 |
  | cudart win-cuda-13.4-x64 | 423.54 |
  | win-cuda-12.4-x64 | 272.12 |
  | cudart win-cuda-12.4-x64 | 391.44 |
  | win-cuda-13.4-arm64 | 147.51 |
  | cudart win-cuda-13.4-arm64 | 153.26 |
  | win-opencl-adreno-arm64 | 13.54 |
  | win-sycl-x64 | 154.10 |
  | win-rocm-10.0-x64 | 263.41 |
  | win-openvino-2026.4.1-x64 | 104.01 |

  Linux builds also exist: CPU, Vulkan, CUDA 12.8/13.4, ROCm, SYCL and OpenVINO.

  — [GitHub API: llama.cpp releases](https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=1); [llama.cpp releases](https://github.com/ggml-org/llama.cpp/releases)
- **Windows AV false positives on llama.cpp builds:**
  - [#26120](https://github.com/ggml-org/llama.cpp/issues/26120): Trojan:Win32/Wacatac.H!ml on b10107 (closed Jul 28, 2026)
  - [#26343](https://github.com/ggml-org/llama.cpp/issues/26343): Defender on the b10195 CPU build
  - [#26739](https://github.com/ggml-org/llama.cpp/issues/26739): **Bitdefender** quarantines `llama-server-impl.dll` in b10299 (Gen:Variant.Ulise)
  - [#27716](https://github.com/ggml-org/llama.cpp/issues/27716): `llama.exe` in the CUDA 12.4 build blocked as Wacatac.B!ml
  - [#27844](https://github.com/ggml-org/llama.cpp/issues/27844): a CUDA 13.3 DLL flagged
  - [#28224](https://github.com/ggml-org/llama.cpp/issues/28224): Defender flags Windows binaries as Wacatac.B!ml (closed Sept 2, 2026)

  — [llama.cpp issues search](https://github.com/ggml-org/llama.cpp/issues?q=antivirus%20OR%20%22false%20positive%22%20OR%20defender)
- **Ollama on Windows:**
  - "Windows 10 22H2 or newer, Home or Pro". "NVIDIA 551.61 or newer Drivers".
  - AMD via ROCm v7 or a Vulkan-capable Radeon driver; Vulkan is enabled by default and is the fallback for some RDNA2 cards.
  - Installs per-user without admin. At least 4 GB for binaries.
  - A standalone `ollama-windows-amd64.zip` exists "for embedding Ollama in other applications". Add-on zips are `-rocm.zip` and `-mlx.zip` (CUDA).
  - Windows ARM64 and Intel GPU are not mentioned.

  — [Ollama docs: Windows](https://docs.ollama.com/windows)
- **Ollama on macOS:** "MacOS Sonoma (v14) or newer". "Apple M series (CPU and GPU support)". "x86 (CPU only)". — [Ollama docs: macOS](https://docs.ollama.com/macos)
- **LM Studio:**
  - "Intel-based Macs are currently not supported". macOS 14.0+. 16 GB+ RAM recommended.
  - Windows x64 (requires AVX2) and ARM, including Snapdragon X Elite. ≥4 GB VRAM recommended.
  - Linux x64/ARM64 AppImage.

  — [LM Studio docs: system requirements](https://lmstudio.ai/docs/app/system-requirements)
- **ort** (docs reference 2.0.0-rc.14):
  - CUDA EP binaries target CUDA ≥ 13.2 and cuDNN ≥ 9.23, compute capability 7.5/8.x/9.x/12.x. Others need ONNX Runtime built from source.
  - CoreML EP (`coreml` feature, e.g. `ComputeUnits::CPUAndNeuralEngine`).
  - DirectML EP (`directml` feature, Windows; needs helper dylibs).
  - WebGPU EP is "experimental and may produce incorrect results/crashes", with binaries for Windows (DX12/11), macOS and Linux.
  - TensorRT EP. WinML, OpenVINO and QNN are not covered on the page.
  - The earlier notes cited ort docs at 2.0.0-rc.15 with ONNX Runtime 1.31.0; the page fetched now shows rc.14. Treat the exact RC as unsettled.

  — [ort docs: execution providers](https://ort.pyke.io/perf/execution-providers); [GitHub: pykeio/ort (earlier notes)](https://github.com/pykeio/ort)
- **ONNX Runtime DirectML EP:**
  - "DirectML is in sustained engineering"; it "continues to be supported". "new feature development has moved to WinML".
  - Requires a DirectX 12 device (NVIDIA Kepler+, AMD GCN1+, Intel Haswell+, Qualcomm Adreno 600+).
  - Memory-pattern optimisation and parallel execution must be disabled. Only one thread may call `Run` on a session at a time.

  — [ONNX Runtime: DirectML EP](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)
- **candle:** CPU (MKL/Accelerate), CUDA, Metal and WASM. Its model list lags. The candle-based tauri-plugin-llm is PolyForm Noncommercial. — [GitHub: huggingface/candle (earlier notes)](https://github.com/huggingface/candle); [CrabNebula tauri-plugin-llm (earlier notes)](https://docs.crabnebula.dev/plugins/llm/)
- **Tauri macOS signing:**
  - Requires a paid Apple Developer account and a **Developer ID Application** certificate for distribution outside the App Store.
  - "Notarization is required when using a Developer ID Application certificate". Use either App Store Connect API keys (`APPLE_API_ISSUER`/`APPLE_API_KEY`/`APPLE_API_KEY_PATH`) or Apple ID (`APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`).
  - Ad-hoc signing (`"-"`) helps on Apple Silicon "where code-signing is required for all apps from the Internet", but users still get a Privacy & Security prompt.
  - The page **does not cover sidecar/externalBin signing**, hardened runtime or entitlements.

  — [Tauri v2: macOS signing](https://v2.tauri.app/distribute/sign/macos/)
- Tauri sidecars need `bundle.externalBin` with a `-$TARGET_TRIPLE` suffix and shell-plugin permissions. — [Tauri v2: sidecar (earlier notes)](https://v2.tauri.app/develop/sidecar/)

### Inferences
- **Sidecar packaging plan (macOS + Windows):**
  - **macOS:** bundle `macos-arm64` (Metal). Optionally bundle `macos-x64` for Intel Macs (~12 MB). Intel-Mac GPU acceleration is not verified; treat it as CPU-class, which matches Ollama's "x86 (CPU only)". Note that LM Studio can't serve Intel Mac users, so /dev/all's own sidecar or Ollama is the only local path there.
  - **Windows x64:** bundle **CPU + Vulkan** (~61 MB together). Vulkan covers NVIDIA, AMD and Intel GPUs reasonably. Download **CUDA 13.4 + cudart (~583 MB)** on demand only after detecting an NVIDIA GPU with a recent driver. Never put CUDA in the base installer.
  - **Windows ARM64 (Snapdragon):** bundle CPU-arm64 (13 MB) + Vulkan-arm64 (25 MB). Optionally offer the OpenCL-Adreno build (14 MB) as a download.
- **AV false positives are a real, current risk.** The llama.cpp Windows binaries now split into `*.exe` + `*-impl.dll` (per issue #26739). Mitigations:
  - Re-sign every sidecar exe and DLL with /dev/all's Authenticode certificate (EV or Azure Trusted Signing helps reputation).
  - Submit builds to the Microsoft Defender false-positive portal.
  - Download GPU variants from a stable, versioned URL with checksum verification.
  - Show a clear UI message if the sidecar is quarantined. Detect a missing exe or DLL after a spawn failure and fall back to "connect to Ollama/LM Studio".
- **macOS notarization:** every Mach-O inside the .app, including sidecars and any downloaded dylibs, must be signed with the hardened runtime for notarization. That is standard Apple practice, though the Tauri page doesn't confirm sidecar handling. Binaries downloaded at runtime (e.g. a newer llama-server) get quarantined unless /dev/all signs and notarizes them itself or clears quarantine for files it fetched. Prefer shipping the Metal llama-server inside the signed bundle and updating it with app updates.
- **ort for embeddings:**
  - macOS: CPU (fast enough for a 270M–600M embedder) or the CoreML EP.
  - Windows: CPU by default. Optionally use DirectML, accepting its "sustained engineering" status and single-thread `Run` limit, or CUDA, which is too heavy to ship for an embedder.
  - Watch for ort WinML support as the forward path.
- **Ollama as a supported backend:** Ollama is a first-class "connect" target on both OSes and uses MLX on Apple Silicon. Its standalone Windows zip makes it technically embeddable, but licence and size review would be needed. Recommend connecting rather than bundling.

### Gaps
- Whether the `macos-x64` llama.cpp build uses Metal on Intel Macs (AMD/Intel iGPU) was not verified.
- Whether Tauri automatically signs `externalBin` sidecars during `tauri build` was not verified, and the docs page is silent.
- ort's support for the WinML or QNN execution providers was not verified.
- Ollama Windows ARM64 support and Intel-GPU (Vulkan) support were not stated in the docs fetched.
- No independent measurement of Vulkan vs CUDA llama.cpp speed on Windows for the recommended models.

## 7. Recommended tiered lineup (macOS + Windows), replacing all Qwen2.5 recommendations

### Takeaway
The corrected Oct 2026 lineup:

| Hardware | (a) General / tool-calling / structured JSON | (b) Code + autocomplete (FIM) | (c) Embeddings |
|---|---|---|---|
| ~8 GB | Qwen3.5-4B or Gemma 4 E4B | Mellum-4b for FIM | EmbeddingGemma-2 (270M) |
| 16 GB (incl. M-series base Macs) | Gemma 4 12B, Qwen3.5-9B or Granite 4.2 8B | Mellum-4b for FIM; Qwen3.5-9B / Mellum2 for code chat | EmbeddingGemma-2 |
| 24–32 GB+ | **Qwen3.8-27B** (or the faster MoE alternatives Qwen3.6-35B-A3B / Gemma 4 26B-A4B) | **Qwen3-Coder-30B-A3B** (FIM + code) | EmbeddingGemma-2 or Qwen3-Embedding-0.6B |
| NVIDIA Windows boxes | Sized by VRAM; see the GPU tiers below | Sized by VRAM | Sized by VRAM |

### Cited Findings
Sizes are approximate Q4 file sizes; the KV cache comes on top.
- Qwen3.5-4B ≈3.4 GB and Qwen3.5-9B ≈6.6 GB (Ollama Q4). Qwen3.5 small: Apache-2.0, 262K context, thinking on by default. — [InsiderLLM (earlier notes)](https://insiderllm.com/guides/best-local-coding-models-2026/); [Artificial Analysis (earlier notes)](https://artificialanalysis.ai/articles/qwen3-5-small-models)
- Gemma 4 QAT Q4_0 weights: E2B 2.9 GB, E4B 4.5 GB, 12B 6.7 GB, 26B-A4B 14.4 GB, 31B 17.5 GB. — [Google AI: Gemma docs (earlier notes)](https://ai.google.dev/gemma/docs/core)
- Gemma 4 12B: Apache-2.0, 256K context, native tool use, Tau2 69.0, LCB v6 72.0. — [HF: gemma-4-12B-it](https://huggingface.co/google/gemma-4-12B-it)
- Granite 4.2 8B: Q4_K_M 5.35 GB, Apache-2.0, tool calling, BFCL v4 52.39. — [HF tree: granite-4.2-8b-GGUF](https://huggingface.co/api/models/ibm-granite/granite-4.2-8b-GGUF/tree/main); [HF: granite-4.2-8b](https://huggingface.co/ibm-granite/granite-4.2-8b)
- Qwen3.8-27B: UD-Q4_K_M 16.46 GB, UD-Q3_K_XL 13.15 GB, UD-IQ3_XXS 10.93 GB. Ollama tag 18 GB. Apache-2.0. — [HF tree: unsloth/Qwen3.8-27B-GGUF](https://huggingface.co/api/models/unsloth/Qwen3.8-27B-GGUF/tree/main); [Ollama: qwen3.8](https://ollama.com/library/qwen3.8)
- Qwen3.6-35B-A3B: UD-Q3_K_M 16.6 GB, UD-Q4_K_M 22.1 GB, ~101.7 tok/s on an RTX 3090. Qwen3.6-27B: Q4_K_M 16.8 GB. — [InsiderLLM (earlier notes)](https://insiderllm.com/guides/best-local-coding-models-2026/)
- Qwen3-Coder-30B-A3B: Q4_K_M 18.56 GB, IQ4_XS 16.38 GB, Q3_K_M 14.71 GB. — [HF tree](https://huggingface.co/api/models/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/tree/main)
- Mellum2-12B-A2.5B-Instruct: Q4_K_M 8.07 GB, Apache-2.0. — [HF tree](https://huggingface.co/api/models/JetBrains/Mellum2-12B-A2.5B-Instruct-GGUF-Q4_K_M/tree/main)
- Nemotron 3.5 Bolt 74B-A9B: "fit in 24GB of memory" (native 2-bit). OpenMDW-1.1. — [HF: Nemotron Bolt](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB)
- Qwen3-Coder-Next 80B-A3B: ≈38 GB at Q4, Apache-2.0, FIM. — [InsiderLLM (earlier notes)](https://insiderllm.com/guides/best-local-coding-models-2026/); [GitHub: QwenLM/Qwen3-Coder](https://github.com/QwenLM/Qwen3-Coder)

### Inferences
These are proposed defaults for the /dev/all curated list. Re-validate each quarter.

**Tier 1: ~8 GB RAM.** Covers 8 GB M1/M2 Macs, 8 GB Windows laptops, CPU or iGPU, Intel Macs.
- (a) General / tools / JSON: **Qwen3.5-4B** (≈3.4 GB) or **Gemma 4 E4B** (≈4.5 GB weights). Use thinking off and grammar/JSON-schema constrained decoding.
- (b) Code / FIM: **JetBrains Mellum-4b** (sft-all or dpo-all, Q4; Apache-2.0; 8K context) for inline completion. Code chat uses the general model.
- (c) Embeddings: **EmbeddingGemma-2 (270M text load)** or Qwen3-Embedding-0.6B. Use CPU via ort or llama-server.
- On 8 GB, run only one LLM at a time. Load the FIM model only while the editor is focused.

**Tier 2: 16 GB.** Covers M-series base Macs (16 GB unified), 16 GB Windows laptops and 6–8 GB VRAM GPUs.
- (a) **Gemma 4 12B** (≈6.7 GB; 256K context; native tools; Apache-2.0) as the default.
  - Alternatives: **Qwen3.5-9B** (≈6.6 GB; strongest vendor BFCL among small models) or **Granite 4.2 8B** (5.35 GB; Apache-2.0; most conservative, enterprise-clean pick).
  - Gemma 4 26B-A4B (14.4 GB) is too tight on a 16 GB Mac.
- (b) FIM: **Mellum-4b**. Code chat: Qwen3.5-9B. Optionally Mellum2-12B-A2.5B-Instruct (8.07 GB, 2.5B active, fast MoE; FIM undocumented).
- (c) **EmbeddingGemma-2**.

**Tier 3: 24–32 GB+ unified memory.** Covers M-series Pro/Max/Ultra and 32 GB+ Windows boxes with partial offload.
- (a) **Qwen3.8-27B** (UD-Q4_K_M 16.5 GB on 32 GB+; UD-Q3_K_XL 13.2 GB on 24 GB). Newest Qwen, Apache-2.0, vision, 262K context.
  - Faster MoE alternatives: **Qwen3.6-35B-A3B** (Q3 16.6 GB / Q4 22.1 GB) or **Gemma 4 26B-A4B** (14.4 GB).
  - Note that 27B dense decodes much slower per token than 3–4B-active MoEs.
- (b) **Qwen3-Coder-30B-A3B-Instruct** (Q4_K_M 18.6 GB or IQ4_XS 16.4 GB) for both FIM and code chat. On 24 GB Macs, use Q3_K_M (14.7 GB) or fall back to Mellum-4b for FIM.
- (c) **EmbeddingGemma-2** (or the 740M multimodal load), or **Qwen3-Embedding-0.6B/4B**. Reranker: **Qwen3-Reranker-0.6B**.
- **64 GB+ Macs:** Qwen3-Coder-Next 80B-A3B (≈38 GB Q4) for agentic code with FIM.

**Tier 4: Windows with an NVIDIA GPU.** Use the llama.cpp CUDA 13.4 sidecar as an on-demand download, or Ollama.
- **8 GB VRAM:**
  - (a) Qwen3.5-9B, Gemma 4 12B or Granite 4.2 8B, fully on GPU.
  - (b) Mellum-4b for FIM.
  - (c) EmbeddingGemma-2 on CPU.
- **12–16 GB VRAM:**
  - (a) Gemma 4 26B-A4B (14.4 GB) or Qwen3.8-27B at UD-IQ3_XXS (10.9 GB) / UD-Q3_K_XL (13.2 GB).
  - (b) Qwen3-Coder-30B-A3B Q3_K_M (14.7 GB, tight on 16 GB) or Mellum-4b.
- **24 GB VRAM** (RTX 3090/4090/5090-class):
  - (a) **Qwen3.8-27B UD-Q4_K_M (16.5 GB)** with room for KV. Alternatives: Qwen3.6-35B-A3B (fast) or **Nemotron 3.5 Bolt 74B-A9B-24GB** (OpenMDW; vLLM-first, GGUF exists; treat as experimental in llama.cpp).
  - (b) **Qwen3-Coder-30B-A3B Q4_K_M (18.6 GB)** for FIM and code.
  - (c) EmbeddingGemma-2.
- **AMD / Intel GPUs on Windows:** same VRAM tiers using the Vulkan build. Expect lower speed than CUDA (not measured here).

**Model-handling rules for all tiers:**
- Disable thinking by default (Qwen3.5/3.8, Granite 4.2, Gemma 4 all support toggles). LFM2.5-24B-A2B "always thinks", so avoid it for latency-sensitive features.
- Always use JSON-schema or grammar constrained decoding, and validate in Rust.
- Prefer Apache-2.0 models as one-click defaults.
- Never ship llama.cpp's `--fim-qwen-1.5b/3b/7b/14b` presets, because they load Qwen2.5.

### Gaps
- No independent, same-harness benchmark compares Gemma 4 12B, Qwen3.5-9B and Granite 4.2 8B on tool calling or JSON validity. All scores above are vendor-reported (e.g. Granite BFCL v4 52.39 vs Qwen3.5-9B's self-reported 66.1, which use different harnesses).
- No measured tokens/sec on base M-series Macs or Windows Vulkan for Qwen3.8-27B, Gemma 4 12B or Qwen3-Coder-30B-A3B.
- Mellum-4b GGUF sizes and real FIM latency were not measured.
- Real-world quality impressions from r/LocalLLaMA could not be gathered, because web search was unavailable this pass.

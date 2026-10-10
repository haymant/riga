//! Local GGUF model manager and inference runtime.
//!
//! Models are downloaded into a per-user data directory, verified against a
//! pinned SHA-256, and then served by an in-process llama.cpp context. The
//! curated catalog is a closed list with pinned hashes, so a mirror cannot
//! silently substitute different weights.

use std::{
    collections::HashMap,
    ffi::CString,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use encoding_rs::UTF_8;
use llama_cpp_2::{
    context::{
        LlamaContext,
        params::{KvCacheType, LlamaContextParams},
    },
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{LlamaChatMessage, LlamaModel, params::LlamaModelParams},
    sampling::LlamaSampler,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// Smallest context we will ask llama.cpp for. Below this the KV cache cannot
/// hold a real system prompt plus a reply.
const MIN_CONTEXT: u32 = 1024;

/// Largest context we will ever ask for, regardless of what a model advertises.
/// This is an upper bound for `RIGA_LOCAL_CONTEXT` and for the auto-fit result;
/// it is not a default. The adaptive load picks the largest window that actually
/// fits the free VRAM, so a small card never lands here.
const MAX_CONTEXT: u32 = 131_072;

/// Default context window, used both as the fallback for a GGUF with no curated
/// entry and as the cap on a curated model's larger window. 16k leaves room for
/// a long reasoning block plus the prompt; the KV cache is q8_0 (see
/// `context_params_with`), so it still fits a consumer GPU. `RIGA_LOCAL_CONTEXT`
/// overrides it.
const DEFAULT_CONTEXT: u32 = 16_384;

/// Tokens decoded per `llama_decode` call. Deliberately not tied to the
/// context size: llama.cpp sizes its compute buffers from `n_ubatch`, so a
/// full-context ubatch on a 32k window would try to allocate tens of gigabytes
/// of activations for one call. The prompt is decoded in chunks of this size.
const MAX_DECODE_BATCH: u32 = 2048;

/// Physical batch. llama.cpp sizes its compute buffers from `n_ubatch`, and a
/// 2048-token ubatch costs about 1 GiB of VRAM — VRAM the auto-fit then cannot
/// give to model layers. 512 keeps the compute buffer small so more layers fit
/// on the GPU; prefill is a little slower but decode (what an agent loop spends
/// its time on) is unaffected.
const MAX_UBATCH: u32 = 512;

/// Compile-time guard on that bound. Tying `n_ubatch` to `n_ctx` is exactly what
/// this decoupling exists to prevent, so it fails the build instead of waiting
/// for a test.
const _: () = assert!(MAX_DECODE_BATCH > 0 && MAX_DECODE_BATCH < MAX_CONTEXT);

/// A capability tier for a local model, used to pick a default run budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Small models (roughly ≤2B): short turns, small windows.
    Compact,
    /// Mid models (roughly 3–8B): the default.
    Middle,
    /// Large or reasoning models (roughly ≥13B): long turns, big windows.
    Large,
}

impl Capability {
    /// Parse a tier name, accepting a few common synonyms.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "compact" | "small" | "tiny" => Some(Self::Compact),
            "middle" | "medium" | "mid" => Some(Self::Middle),
            "large" | "big" => Some(Self::Large),
            _ => None,
        }
    }

    /// The default run budget for this tier.
    pub fn budget(self) -> LocalBudget {
        match self {
            Self::Compact => LocalBudget {
                max_tokens: 2_048,
                min_output: 1_024,
                run_token_budget: 8_192,
                max_turns: 6,
                turn_seconds: 150,
                no_progress_seconds: 60,
            },
            Self::Middle => LocalBudget {
                max_tokens: 8_192,
                min_output: 1_536,
                run_token_budget: 20_480,
                max_turns: 8,
                turn_seconds: 240,
                no_progress_seconds: 90,
            },
            Self::Large => LocalBudget {
                max_tokens: 16_384,
                min_output: 2_048,
                run_token_budget: 40_960,
                max_turns: 10,
                turn_seconds: 360,
                no_progress_seconds: 120,
            },
        }
    }
}

/// Run-scoped local-model budget. The `set_model_budget` tool adjusts it so a
/// prompt can pick a capability tier, resize a step, or set exact numbers; the
/// local loop re-reads it every turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBudget {
    /// Output tokens requested per turn (clamped to the context room).
    pub max_tokens: u32,
    /// Output tokens reserved when trimming the prompt, so a turn always has
    /// room to finish a reasoning block and emit a tool call or an answer.
    pub min_output: u32,
    /// Tokens across every turn of the run.
    pub run_token_budget: usize,
    /// Turns in one run.
    pub max_turns: usize,
    /// Per-turn wall clock, seconds.
    pub turn_seconds: u64,
    /// Idle window before a turn is treated as stalled, seconds.
    pub no_progress_seconds: u64,
}

impl Default for LocalBudget {
    fn default() -> Self {
        Capability::Middle.budget()
    }
}

/// Hard ceilings the budget can never exceed, so "make it bigger" cannot turn
/// one run into an hour of decoding.
const MAX_BUDGET_TOKENS: u32 = 32_768;
const MAX_RUN_TOKENS: usize = 131_072;
const MAX_TURNS: usize = 24;
const MAX_TURN_SECONDS: u64 = 900;
const MAX_IDLE_SECONDS: u64 = 300;

impl LocalBudget {
    /// Grow the budget for a step that needs more room (roughly 2x), capped.
    pub fn enlarged(self) -> Self {
        Self {
            max_tokens: self.max_tokens.saturating_mul(2).min(MAX_BUDGET_TOKENS),
            min_output: self.min_output.saturating_mul(2).min(MAX_BUDGET_TOKENS),
            run_token_budget: self.run_token_budget.saturating_mul(2).min(MAX_RUN_TOKENS),
            max_turns: (self.max_turns + 4).min(MAX_TURNS),
            turn_seconds: self.turn_seconds.saturating_mul(2).min(MAX_TURN_SECONDS),
            no_progress_seconds: self
                .no_progress_seconds
                .saturating_mul(2)
                .min(MAX_IDLE_SECONDS),
        }
    }

    /// Shrink the budget for a cheap step (roughly half), floored at compact.
    pub fn shrunk(self) -> Self {
        let floor = Capability::Compact.budget();
        Self {
            max_tokens: (self.max_tokens / 2).max(floor.max_tokens),
            min_output: (self.min_output / 2).max(floor.min_output),
            run_token_budget: (self.run_token_budget / 2).max(floor.run_token_budget),
            max_turns: self.max_turns.saturating_sub(4).max(floor.max_turns),
            turn_seconds: (self.turn_seconds / 2).max(floor.turn_seconds),
            no_progress_seconds: (self.no_progress_seconds / 2).max(floor.no_progress_seconds),
        }
    }

    /// Clamp every field to the hard limits.
    pub fn clamped(self) -> Self {
        let floor = Capability::Compact.budget();
        Self {
            max_tokens: self.max_tokens.clamp(floor.max_tokens, MAX_BUDGET_TOKENS),
            min_output: self.min_output.clamp(floor.min_output, MAX_BUDGET_TOKENS),
            run_token_budget: self
                .run_token_budget
                .clamp(floor.run_token_budget, MAX_RUN_TOKENS),
            max_turns: self.max_turns.clamp(floor.max_turns, MAX_TURNS),
            turn_seconds: self
                .turn_seconds
                .clamp(floor.turn_seconds, MAX_TURN_SECONDS),
            no_progress_seconds: self
                .no_progress_seconds
                .clamp(floor.no_progress_seconds, MAX_IDLE_SECONDS),
        }
    }

    /// Apply a `set_model_budget` payload: an optional `tier`, optional
    /// `enlarge`/`shrink`, then explicit field overrides, all clamped.
    pub fn apply(&mut self, input: &serde_json::Value) -> Result<String, String> {
        let mut next = *self;
        if let Some(tier) = input.get("tier").and_then(serde_json::Value::as_str) {
            next = Capability::parse(tier)
                .ok_or_else(|| {
                    format!("unknown capability tier `{tier}`; use compact, middle, or large")
                })?
                .budget();
        }
        if input
            .get("enlarge")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            next = next.enlarged();
        }
        if input
            .get("shrink")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            next = next.shrunk();
        }
        if let Some(value) = input.get("max_tokens").and_then(serde_json::Value::as_u64) {
            next.max_tokens = value.min(u32::MAX as u64) as u32;
        }
        if let Some(value) = input.get("min_output").and_then(serde_json::Value::as_u64) {
            next.min_output = value.min(u32::MAX as u64) as u32;
        }
        if let Some(value) = input
            .get("run_token_budget")
            .and_then(serde_json::Value::as_u64)
        {
            next.run_token_budget = value as usize;
        }
        if let Some(value) = input.get("max_turns").and_then(serde_json::Value::as_u64) {
            next.max_turns = value as usize;
        }
        if let Some(value) = input
            .get("turn_seconds")
            .and_then(serde_json::Value::as_u64)
        {
            next.turn_seconds = value;
        }
        if let Some(value) = input
            .get("no_progress_seconds")
            .and_then(serde_json::Value::as_u64)
        {
            next.no_progress_seconds = value;
        }
        *self = next.clamped();
        Ok(self.describe())
    }

    /// A one-line summary the tool returns to the model.
    pub fn describe(&self) -> String {
        format!(
            "max_tokens={} min_output={} run_token_budget={} max_turns={} turn_seconds={} no_progress_seconds={}",
            self.max_tokens,
            self.min_output,
            self.run_token_budget,
            self.max_turns,
            self.turn_seconds,
            self.no_progress_seconds
        )
    }
}

/// Device memory the auto-fit planner is told to leave unused, per device.
///
/// llama.cpp's estimate of weights + KV cache is exact for the tensors it knows
/// about but not for the compute buffer it allocates when the context is created
/// (about 1 GiB at the default ubatch), driver pools, or another process taking
/// VRAM between the fit and the load. 1.5 GiB reserves room for the compute
/// buffer plus fragmentation; a tighter margin produced a plan that offloaded
/// too many layers and then stalled at context creation.
const FIT_MARGIN_BYTES: usize = 1536 * 1024 * 1024;

/// Minimum wall-clock gap between download progress broadcasts. Ten per second
/// is past the point where a progress bar looks continuous.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

const CATALOG: &[CuratedModel] = &[
    CuratedModel {
        id: "qwen3-4b-instruct",
        name: "Qwen3 4B",
        file_name: "Qwen_Qwen3-4B-Q4_K_M.gguf",
        download_url: "https://huggingface.co/bartowski/Qwen_Qwen3-4B-GGUF/resolve/main/Qwen_Qwen3-4B-Q4_K_M.gguf",
        size_bytes: 2_497_280_960,
        sha256: "fbe1d5edd4ce802ae3ae7c7e4ab7d09789d697fdac1fc7929f8df4ca3c41bae3",
        max_context: 32_768,
        license_url: "https://huggingface.co/Qwen/Qwen3-4B",
        reasoning: Some(ReasoningTags {
            open: "\u{3c}think\u{3e}",
            close: "\u{3c}/think\u{3e}",
        }),
    },
    CuratedModel {
        id: "qwen2.5-coder-7b-instruct",
        name: "Qwen2.5 Coder 7B Instruct",
        file_name: "Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        download_url: "https://huggingface.co/bartowski/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        size_bytes: 4_683_074_336,
        sha256: "1664fccab734674a50763490a8c6931b70e3f2f8ec10031b54806d30e5f956b6",
        max_context: 32_768,
        license_url: "https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct",
        reasoning: None,
    },
    CuratedModel {
        id: "phi-4-mini-instruct",
        name: "Phi-4 Mini 3.8B Instruct",
        file_name: "microsoft_Phi-4-mini-instruct-Q4_K_M.gguf",
        download_url: "https://huggingface.co/bartowski/microsoft_Phi-4-mini-instruct-GGUF/resolve/main/microsoft_Phi-4-mini-instruct-Q4_K_M.gguf",
        size_bytes: 2_491_874_688,
        sha256: "01999f17c39cc3074afae5e9c539bc82d45f2dd7faa3917c66cbef76fce8c0c2",
        max_context: 131_072,
        license_url: "https://huggingface.co/microsoft/Phi-4-mini-instruct",
        reasoning: None,
    },
    CuratedModel {
        id: "hermes-3-llama-3.1-8b",
        name: "Hermes 3 8B (Llama 3.1)",
        file_name: "Hermes-3-Llama-3.1-8B-Q4_K_M.gguf",
        download_url: "https://huggingface.co/bartowski/Hermes-3-Llama-3.1-8B-GGUF/resolve/main/Hermes-3-Llama-3.1-8B-Q4_K_M.gguf",
        size_bytes: 4_920_734_624,
        sha256: "44b5c529dffaf34657a4c44a7919e5958b4270fbd365eed3b87d3444b45a378a",
        max_context: 131_072,
        license_url: "https://huggingface.co/NousResearch/Hermes-3-Llama-3.1-8B",
        reasoning: None,
    },
];

#[derive(Clone, Copy)]
struct CuratedModel {
    id: &'static str,
    name: &'static str,
    file_name: &'static str,
    download_url: &'static str,
    size_bytes: u64,
    sha256: &'static str,
    /// The model's real context window, read from the `*.context_length` key of
    /// the GGUF this entry downloads. These are not guesses from the model card:
    /// `Qwen/Qwen3-4B` ships a 40960-token `config.json` but a 32768-token GGUF,
    /// and the GGUF is what llama.cpp reads.
    max_context: u32,
    license_url: &'static str,
    /// Explicit reasoning dialect, when the template alone is not enough. `None`
    /// means "read it from the chat template" (see `ReasoningFormat`).
    reasoning: Option<ReasoningTags>,
}

/// How a model surfaces its reasoning.
///
/// Resolved once per loaded model so neither the streamer nor the final-answer
/// stripper hard-codes one model's tags. `open`/`close` empty means the model has
/// no reasoning channel and no block is shown. A curated entry can declare the
/// tags explicitly (its `reasoning` field); otherwise they are read from the chat
/// template, so a manually copied GGUF whose template reveals its dialect works
/// with no code change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReasoningFormat {
    /// Tag the model emits to open a reasoning block, e.g. ` thinking`.
    pub open: String,
    /// Tag that closes it, e.g. ``.
    pub close: String,
}

/// Tag pairs the loader recognises in a chat template, most specific first.
const REASONING_DIALECTS: &[(&str, &str)] = &[
    ("\u{3c}think\u{3e}", "\u{3c}/think\u{3e}"),
    ("\u{3c}thinking\u{3e}", "\u{3c}/thinking\u{3e}"),
    ("\u{3c}reasoning\u{3e}", "\u{3c}/reasoning\u{3e}"),
    ("[THINK]", "[/THINK]"),
];

impl ReasoningFormat {
    /// A model with no reasoning channel: nothing is shown and nothing stripped.
    pub fn none() -> Self {
        Self::default()
    }

    /// The Qwen3/DeepSeek-style ` thinking…` dialect.
    pub fn think() -> Self {
        Self {
            open: "\u{3c}think\u{3e}".to_string(),
            close: "\u{3c}/think\u{3e}".to_string(),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.open.is_empty() && !self.close.is_empty()
    }

    /// Read the dialect from a chat template. The template is the source of truth
    /// for a model we have not curated: if it names a known tag pair, the model
    /// speaks that dialect.
    pub fn detect_from_template(template: &str) -> Self {
        for (open, close) in REASONING_DIALECTS {
            if template.contains(open) && template.contains(close) {
                return Self {
                    open: (*open).to_string(),
                    close: (*close).to_string(),
                };
            }
        }
        Self::none()
    }

    /// Whether the formatted prompt already opened the block, so the model's
    /// first tokens are reasoning with no opening tag of its own.
    pub fn opens_in_prompt(&self, prompt: &str) -> bool {
        self.enabled() && prompt.trim_end().ends_with(self.open.as_str())
    }

    /// Remove every reasoning block from a reply. An unterminated block (a turn
    /// cut off mid-thought) drops the tail rather than leaking half of it.
    pub fn strip(&self, text: &str) -> String {
        if !self.enabled() {
            return text.trim().to_owned();
        }
        let (open, close) = (self.open.as_str(), self.close.as_str());
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        // A template-opened block carries no opening tag in the generated text:
        // the reply starts inside the block, so the first marker is the close tag.
        if let Some(end) = rest.find(close)
            && rest.find(open).is_none_or(|open_at| end < open_at)
        {
            rest = &rest[end + close.len()..];
        }
        while let Some(start) = rest.find(open) {
            out.push_str(&rest[..start]);
            let after = &rest[start + open.len()..];
            match after.find(close) {
                Some(end) => rest = &after[end + close.len()..],
                None => return out.trim().to_owned(),
            }
        }
        out.push_str(rest);
        out.trim().to_owned()
    }
}

/// A curated model's explicit reasoning dialect, when the template alone is not
/// enough to describe it.
#[derive(Clone, Copy)]
struct ReasoningTags {
    open: &'static str,
    close: &'static str,
}

impl ReasoningTags {
    fn to_format(self) -> ReasoningFormat {
        ReasoningFormat {
            open: self.open.to_string(),
            close: self.close.to_string(),
        }
    }
}

/// Apply a `RIGA_LOCAL_REASONING` override (`auto`, `on`, `off`) to the detected
/// dialect, so the display can be forced on or off without a rebuild.
fn parse_reasoning_override(value: Option<&str>, detected: ReasoningFormat) -> ReasoningFormat {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("off" | "none" | "0" | "false") => ReasoningFormat::none(),
        Some("on" | "think" | "1" | "true") => {
            if detected.enabled() {
                detected
            } else {
                ReasoningFormat::think()
            }
        }
        _ => detected,
    }
}

/// Resolve the context window to actually use.
///
/// `trained` is the GGUF's own `*.context_length` and is authoritative — it
/// comes from the file on disk, so a curated entry that disagrees with the
/// download cannot talk us into allocating a window the weights do not support.
fn resolve_context(trained: u32, wanted: u32) -> u32 {
    // Clamp against u32::MAX first so a zero/unknown `trained` cannot make the
    // `min` below fall under MIN_CONTEXT, which would panic `clamp`.
    let ceiling = trained.clamp(MIN_CONTEXT, u32::MAX).min(MAX_CONTEXT);
    wanted.clamp(MIN_CONTEXT, ceiling)
}

/// Parse a `RIGA_LOCAL_CONTEXT` override into a positive token count. Absent,
/// junk, or zero values return `None` so the caller falls back to the default.
fn parse_context_override(value: Option<&str>) -> Option<u32> {
    value
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
}

/// The fallback context window: used when `RIGA_LOCAL_CONTEXT` is unset and as
/// the CPU-fallback window when no GPU plan fits.
///
/// A curated entry carries its own window; `RIGA_LOCAL_CONTEXT` overrides it so
/// a machine with a low memory ceiling — or a pressure-based OOM killer — can
/// trade context for KV-cache headroom. The GGUF's trained window still clamps
/// the result in `resolve_context`, so this can never exceed what the weights
/// support. The live default is chosen by `fit_params` (see the load path), not
/// here.
fn requested_context(curated_max: Option<u32>, override_value: Option<&str>) -> u32 {
    parse_context_override(override_value)
        // Cap the default at `DEFAULT_CONTEXT` and let `RIGA_LOCAL_CONTEXT` raise
        // it: a curated model may advertise 128k, but a window that large does not
        // fit a consumer GPU and decode crawls.
        .unwrap_or_else(|| curated_max.unwrap_or(DEFAULT_CONTEXT).min(DEFAULT_CONTEXT))
}

/// Context params for the KV cache.
///
/// On a GPU build FlashAttention is enabled and the KV cache is q8_0: quantized V
/// requires FlashAttention, and q8_0 roughly halves the cache versus f16 with
/// negligible quality loss, so a 16k window fits where f16 would force fewer
/// offloaded layers. A CPU-only build keeps the defaults.
fn context_params_with(n_ctx: u32, batch_size: u32) -> LlamaContextParams {
    let params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(n_ctx))
        .with_n_batch(batch_size)
        .with_n_ubatch(batch_size.min(MAX_UBATCH));
    if cfg!(any(feature = "cuda", feature = "metal")) {
        params
            .with_flash_attention_policy(llama_cpp_sys_2::LLAMA_FLASH_ATTN_TYPE_ENABLED)
            .with_type_k(KvCacheType::Q8_0)
            .with_type_v(KvCacheType::Q8_0)
    } else {
        params
    }
}

/// Clamp a requested completion to the room the prompt leaves behind.
///
/// Auto-shrinking rather than rejecting is what keeps the guarantee the batch
/// fix was about: whatever we return still fits the context, so generation can
/// never run off the end of the KV cache. Only a prompt that fills the context
/// outright is an error, because there is no valid completion left to make.
fn resolve_max_tokens(
    requested: u32,
    prompt_tokens: u32,
    context_size: u32,
) -> Result<u32, String> {
    let room = context_size.saturating_sub(prompt_tokens);
    if room == 0 {
        return Err(format!(
            "The prompt already fills the {context_size}-token context; start a new session or reduce history"
        ));
    }
    Ok(requested.clamp(1, room))
}

struct LoadedModel {
    // Drop the model before its backend.
    model: LlamaModel,
    backend: Arc<LlamaBackend>,
    /// The resolved window: the model's own maximum, capped by MAX_CONTEXT.
    /// Resolved once at load time so inference does not repeat the clamping.
    context_size: u32,
    /// How this model tags reasoning, resolved once at load time.
    reasoning: ReasoningFormat,
}

#[derive(Default)]
pub struct LocalModelRuntime {
    /// `LlamaBackend::init` may only run once per process, so it is created
    /// lazily here and shared across every model (re)load.
    backend: OnceLock<Arc<LlamaBackend>>,
    engine: Arc<Mutex<Option<LoadedModel>>>,
    /// The loaded model's file name, mirrored here so status checks never have
    /// to lock `engine`. A generation holds `engine` for its whole (minutes-long)
    /// decode, and status checks run on async workers; locking there could park
    /// one worker per check behind a running model.
    loaded_name: Arc<Mutex<Option<String>>>,
    /// The loaded model's reasoning dialect, mirrored like `loaded_name` so the
    /// UI can read it without locking `engine` behind a running generation.
    loaded_reasoning: Arc<Mutex<ReasoningFormat>>,
    downloads: Arc<Mutex<HashMap<String, CancellationToken>>>,
    generations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    /// Broadcast sink for download progress and terminal download state.
    events: Arc<Mutex<Option<tokio::sync::broadcast::Sender<LocalModelEvent>>>>,
}

impl LocalModelRuntime {
    fn models_dir(&self) -> PathBuf {
        models_root()
    }

    /// The managed models directory, for the UI's "reveal folder" affordance.
    pub fn models_dir_public(&self) -> String {
        self.models_dir().to_string_lossy().into_owned()
    }

    /// Subscribe before starting a download so the first progress event is not
    /// missed. Returns a receiver the caller should keep draining.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<LocalModelEvent> {
        let mut guard = self.events.lock().expect("local model events lock");
        if guard.is_none() {
            *guard = Some(tokio::sync::broadcast::channel(256).0);
        }
        guard
            .as_ref()
            .expect("local model event channel")
            .subscribe()
    }

    fn emit(&self, event: LocalModelEvent) {
        if let Ok(guard) = self.events.lock()
            && let Some(sender) = guard.as_ref()
        {
            // A send error only means nobody is listening, which is fine.
            let _ = sender.send(event);
        }
    }

    pub fn curated_catalog(&self) -> Vec<CuratedModelEntry> {
        CATALOG
            .iter()
            .map(|model| CuratedModelEntry {
                id: model.id.into(),
                name: model.name.into(),
                file_name: model.file_name.into(),
                size_bytes: model.size_bytes,
                sha256: model.sha256.into(),
                // Report the window the model will really run at, so the picker's
                // "context N" label matches inference.
                recommended_context: resolve_context(model.max_context, model.max_context),
                max_context: model.max_context,
                quant: "Q4_K_M",
                license_url: model.license_url,
            })
            .collect()
    }

    pub fn list_installed(&self) -> Result<Vec<InstalledModel>, String> {
        Self::list_models_in(&self.models_dir())
    }

    /// Scan `directory` for GGUF files, matching curated entries by file name and
    /// listing anything else as a user-provided model.
    pub(crate) fn list_models_in(
        directory: &std::path::Path,
    ) -> Result<Vec<InstalledModel>, String> {
        let mut models = Vec::new();
        let entries = fs::read_dir(directory)
            .map_err(|error| format!("Could not scan models folder: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if !path.is_file() || !is_gguf(&path) {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            let curated = CATALOG.iter().find(|model| model.file_name == file_name);
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            models.push(InstalledModel {
                id: curated
                    .map(|model| model.id.to_string())
                    .unwrap_or_else(|| file_name.clone()),
                name: curated
                    .map(|model| model.name.to_string())
                    .unwrap_or_else(|| file_name.clone()),
                file_name: file_name.clone(),
                path: path.to_string_lossy().into_owned(),
                size_bytes: metadata.len(),
                curated: curated.is_some(),
                recommended_context: curated
                    .map(|model| resolve_context(model.max_context, model.max_context)),
                license_url: curated.map(|model| model.license_url.to_string()),
            });
        }
        models.sort_by_key(|model| model.name.to_lowercase());
        Ok(models)
    }

    pub fn loaded_file_name(&self) -> Option<String> {
        self.loaded_name.lock().ok().and_then(|guard| guard.clone())
    }

    /// How the loaded model tags reasoning. Mirrored at load time so callers can
    /// read it without locking `engine`, which a running generation holds.
    pub fn reasoning_format(&self) -> ReasoningFormat {
        self.loaded_reasoning
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| ReasoningFormat::none())
    }

    /// Whether `model_id` names a curated entry, and whether it is already
    /// transferring.
    ///
    /// Both are checked *before* a download is accepted, because the request
    /// returns as soon as the transfer is spawned. Validating inside the spawned
    /// task would report success for a bad id and leave the UI waiting on a
    /// progress bar that will never move.
    pub fn download_blocker(&self, model_id: &str) -> Option<String> {
        if !CATALOG.iter().any(|model| model.id == model_id) {
            return Some(format!("Unknown curated model: {model_id}"));
        }
        let jobs = self.downloads.lock().ok()?;
        if jobs.contains_key(model_id) {
            return Some("This model is already downloading".into());
        }
        None
    }

    /// Begin a download. Returns immediately; progress arrives on the event
    /// broadcast and the terminal state resolves through the task handle.
    pub async fn start_download(&self, model_id: &str) -> Result<(), String> {
        if let Some(blocker) = self.download_blocker(model_id) {
            return Err(blocker);
        }
        let model = CATALOG
            .iter()
            .find(|candidate| candidate.id == model_id)
            .copied()
            .ok_or("Unknown curated model")?;
        let cancel = CancellationToken::new();
        {
            let mut jobs = self
                .downloads
                .lock()
                .map_err(|_| "Download state is unavailable")?;
            if jobs.contains_key(model_id) {
                return Err("This model is already downloading".into());
            }
            jobs.insert(model_id.to_owned(), cancel.clone());
        }
        let directory = self.models_dir();
        let partial_path = directory.join(format!("{}.part", model.file_name));
        let final_path = directory.join(model.file_name);
        // From here on the job is registered, so any failure has to publish a
        // terminal event: the request already returned 202 and the picker is
        // now rendering a progress bar that only a terminal event can resolve.
        let client = match reqwest::Client::builder()
            .user_agent("riga-server/0.1 local model downloader")
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                return Err(
                    self.fail_download(model_id, format!("Could not start download: {error}"))
                );
            }
        };
        let mut response = match client.get(model.download_url).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => response,
                Err(error) => {
                    return Err(self.fail_download(
                        model_id,
                        format!("Model host returned an error: {error}"),
                    ));
                }
            },
            Err(error) => {
                return Err(
                    self.fail_download(model_id, format!("Download request failed: {error}"))
                );
            }
        };
        let mut file = match tokio::fs::File::create(&partial_path).await {
            Ok(file) => file,
            Err(error) => {
                return Err(self.fail_download(
                    model_id,
                    format!("Could not create partial model file: {error}"),
                ));
            }
        };
        let mut hasher = Sha256::new();
        let mut downloaded = 0u64;
        // Progress is broadcast once per response chunk, and a fast host on a
        // multi-gigabyte file delivers thousands of them. At that rate the
        // browser re-renders the picker thousands of times for a bar that moves
        // 0.1% between frames, so emission is capped to something a human can
        // actually perceive. The terminal event is separate, so the final frame
        // being skipped costs nothing.
        let mut last_progress = tokio::time::Instant::now() - PROGRESS_INTERVAL;
        // Any early exit has to do three things: drop the handle, delete the
        // truncated file, and publish a terminal event. A bare `return Err`
        // left a partial .part on disk for good and, worse, never released the
        // job slot, so that model could never be downloaded again and the
        // picker's progress bar never resolved.
        macro_rules! abandon {
            ($message:expr) => {{
                drop(file);
                let _ = tokio::fs::remove_file(&partial_path).await;
                return Err(self.fail_download(model_id, $message));
            }};
        }
        // A local enum rather than letting the `abandon!` arm diverge, which
        // would leave the select's result type as an inference variable.
        enum Step<T> {
            Chunk(T),
            End,
            Cancel,
            Failed(String),
        }
        loop {
            let step = tokio::select! {
                _ = cancel.cancelled() => Step::Cancel,
                chunk = response.chunk() => match chunk {
                    Ok(Some(chunk)) => Step::Chunk(chunk),
                    Ok(None) => Step::End,
                    Err(error) => Step::Failed(format!("Model download interrupted: {error}")),
                },
            };
            let chunk = match step {
                Step::Chunk(chunk) => chunk,
                Step::End => break,
                Step::Cancel => abandon!("Download cancelled".to_owned()),
                Step::Failed(message) => abandon!(message),
            };
            downloaded += chunk.len() as u64;
            hasher.update(&chunk);
            use tokio::io::AsyncWriteExt;
            if let Err(error) = file.write_all(&chunk).await {
                abandon!(format!("Could not write model data: {error}"));
            }
            let now = tokio::time::Instant::now();
            if now.duration_since(last_progress) >= PROGRESS_INTERVAL {
                last_progress = now;
                self.emit(LocalModelEvent::DownloadProgress(DownloadProgress {
                    model_id: model_id.to_owned(),
                    downloaded_bytes: downloaded,
                    total_bytes: model.size_bytes,
                    percent: (downloaded as f64 / model.size_bytes as f64 * 100.0).min(100.0),
                }));
            }
        }
        use tokio::io::AsyncWriteExt;
        file.flush().await.map_err(|error| error.to_string())?;
        file.sync_all().await.map_err(|error| error.to_string())?;
        drop(file);
        if downloaded != model.size_bytes {
            let _ = tokio::fs::remove_file(&partial_path).await;
            let message = format!(
                "Downloaded size mismatch: expected {} bytes, received {downloaded}",
                model.size_bytes
            );
            self.emit(LocalModelEvent::DownloadFailed {
                model_id: model_id.to_owned(),
                message: message.clone(),
            });
            self.clear_download(model_id);
            return Err(message);
        }
        let actual_hash = format!("{:x}", hasher.finalize());
        if actual_hash != model.sha256 {
            let _ = tokio::fs::remove_file(&partial_path).await;
            let message =
                "Model SHA-256 verification failed; the incomplete file was discarded".to_owned();
            self.emit(LocalModelEvent::DownloadFailed {
                model_id: model_id.to_owned(),
                message: message.clone(),
            });
            self.clear_download(model_id);
            return Err(message);
        }
        tokio::fs::rename(&partial_path, &final_path)
            .await
            .map_err(|error| format!("Could not finalize model file: {error}"))?;
        self.emit(LocalModelEvent::DownloadFinished {
            model_id: model_id.to_owned(),
            path: final_path.to_string_lossy().into_owned(),
        });
        self.clear_download(model_id);
        Ok(())
    }

    fn clear_download(&self, model_id: &str) {
        if let Ok(mut jobs) = self.downloads.lock() {
            jobs.remove(model_id);
        }
    }

    /// Publish a terminal failure and release the job slot, returning the
    /// message so callers can `return Err(self.fail_download(..))`.
    ///
    /// A download is only terminal-once: the picker's progress bar is resolved
    /// by the event, not by the response, so a failure that does not publish
    /// leaves the UI spinning forever.
    fn fail_download(&self, model_id: &str, message: String) -> String {
        self.emit(LocalModelEvent::DownloadFailed {
            model_id: model_id.to_owned(),
            message: message.clone(),
        });
        self.clear_download(model_id);
        message
    }

    pub fn cancel_download(&self, model_id: &str) -> Result<(), String> {
        let jobs = self
            .downloads
            .lock()
            .map_err(|_| "Download state is unavailable")?;
        let Some(cancel) = jobs.get(model_id) else {
            return Err("No active download for this model".into());
        };
        cancel.cancel();
        Ok(())
    }

    /// Confine a caller-supplied path to the managed models directory. Without
    /// this, `load_model` would read any file the process can open.
    fn allowed_model_path(&self, path: &str) -> Result<PathBuf, String> {
        let root = self
            .models_dir()
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let candidate = Path::new(path)
            .canonicalize()
            .map_err(|error| format!("Model file not found: {error}"))?;
        if !candidate.starts_with(&root) || !candidate.is_file() || !is_gguf(&candidate) {
            return Err("Choose a .gguf file inside the RIGA models folder".into());
        }
        Ok(candidate)
    }

    pub async fn load_model(&self, path: &str) -> Result<(), String> {
        let path = self.allowed_model_path(path)?;
        let model_path = path.clone();
        // Initialize the process-wide llama.cpp backend at most once. Reusing
        // the existing handle is what lets a second `load` succeed instead of
        // failing with `BackendAlreadyInitialized`.
        let backend = match self.backend.get() {
            Some(backend) => backend.clone(),
            None => {
                let mut initialized = LlamaBackend::init()
                    .map_err(|error| format!("Could not initialize llama.cpp: {error}"))?;
                // llama.cpp writes tensor-loader diagnostics directly to stderr.
                // That stream is the terminal while the CLI owns the alternate
                // screen, so forwarding it would corrupt the TUI. Runtime errors
                // are returned through the normal Rust result path instead.
                initialized.void_logs();
                let candidate = Arc::new(initialized);
                let _ = self.backend.set(candidate);
                self.backend
                    .get()
                    .ok_or("Could not initialize llama.cpp: backend unavailable")?
                    .clone()
            }
        };
        let engine = self.engine.clone();
        let loaded_name = self.loaded_name.clone();
        let loaded_reasoning = self.loaded_reasoning.clone();
        tokio::task::spawn_blocking(move || {
            let file_name = model_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("local model")
                .to_owned();
            let curated = CATALOG.iter().find(|entry| entry.file_name == file_name);

            // The window to run with: `RIGA_LOCAL_CONTEXT` if set, else the capped
            // default. `fit_params` reduces GPU layers to fit it rather than
            // shrinking the window, so the model never lands on CPU with a huge
            // KV cache — which is what `n_ctx = 0` auto-select can do.
            let requested = requested_context(
                curated.map(|entry| entry.max_context),
                std::env::var("RIGA_LOCAL_CONTEXT").ok().as_deref(),
            )
            .clamp(MIN_CONTEXT, MAX_CONTEXT);
            let batch_size = requested.min(MAX_DECODE_BATCH);
            let mut context_params = context_params_with(requested, batch_size);

            // Fit the GPU plan to the memory actually available instead of
            // offloading every layer (`u32::MAX`), which OOMs any card that
            // cannot hold the whole model plus its KV cache. `fit_params` sets
            // `n_gpu_layers`, the tensor split and any buffer overrides in place;
            // it requires `n_gpu_layers` to still be at its default, so the
            // params are left untouched until here. On a CPU-only build this
            // resolves to zero GPU layers and is harmless.
            let model_path_c = CString::new(
                model_path
                    .to_str()
                    .ok_or_else(|| "Model path is not valid UTF-8".to_string())?,
            )
            .map_err(|_| "Model path contains an interior NUL byte".to_string())?;
            let mut margins = vec![FIT_MARGIN_BYTES; llama_cpp_2::max_devices().max(1)];
            let mut params = Box::pin(LlamaModelParams::default());
            let fit = params.as_mut().fit_params(
                model_path_c.as_c_str(),
                &mut context_params,
                &mut margins,
                MIN_CONTEXT,
                llama_cpp_sys_2::GGML_LOG_LEVEL_WARN,
            );
            match fit {
                Ok(result) => tracing::info!(
                    model = %file_name,
                    n_gpu_layers = params.n_gpu_layers(),
                    context = result.n_ctx,
                    "local model GPU plan fitted to available memory"
                ),
                Err(error) => {
                    // No plan fit the free device memory (or fitting failed).
                    // Fall back to CPU-only rather than refusing to load, so the
                    // model still runs — just without GPU offload.
                    tracing::warn!(
                        %error,
                        model = %file_name,
                        "local model auto-fit found no GPU plan; loading on CPU"
                    );
                    params = Box::pin(LlamaModelParams::default().with_n_gpu_layers(0));
                }
            }
            let model = LlamaModel::load_from_file(&backend, &model_path, &params).map_err(
                |error| {
                    format!("Model could not be loaded (the GGUF may be corrupt or exceed available memory): {error}")
                },
            )?;
            // `fit_params` auto-selects `n_ctx` when it was left unset and leaves
            // an explicit value alone; either way this is the window to use.
            let fitted_context = context_params
                .n_ctx()
                .map(std::num::NonZeroU32::get)
                .unwrap_or(requested);
            // The GGUF header is the source of truth; the curated window and the
            // fitted window are only starting points that get clamped by it.
            let context_size = resolve_context(model.n_ctx_train(), fitted_context);
            // Report the window actually resolved, so a short output budget can
            // be diagnosed against the real context instead of guessed at.
            tracing::info!(
                model = %file_name,
                context_size,
                trained = model.n_ctx_train(),
                "local model loaded"
            );
            // Resolve the reasoning dialect: a curated override wins, else read
            // the tags out of the model's own chat template, else none. A
            // `RIGA_LOCAL_REASONING` env value then forces it on or off.
            let template = model
                .chat_template(None)
                .ok()
                .and_then(|template| template.to_str().ok().map(str::to_owned));
            let detected = curated
                .and_then(|entry| entry.reasoning)
                .map(ReasoningTags::to_format)
                .unwrap_or_else(|| {
                    ReasoningFormat::detect_from_template(template.as_deref().unwrap_or_default())
                });
            let reasoning = parse_reasoning_override(
                std::env::var("RIGA_LOCAL_REASONING").ok().as_deref(),
                detected,
            );
            tracing::info!(
                model = %file_name,
                reasoning = reasoning.enabled(),
                open = %reasoning.open,
                "local model reasoning dialect resolved"
            );
            let loaded = LoadedModel {
                model,
                backend,
                context_size,
                reasoning: reasoning.clone(),
            };
            *engine
                .lock()
                .map_err(|_| "Model engine state is unavailable")? = Some(loaded);
            if let Ok(mut guard) = loaded_name.lock() {
                *guard = Some(file_name);
            }
            if let Ok(mut guard) = loaded_reasoning.lock() {
                *guard = reasoning;
            }
            Ok::<(), String>(())
        })
        .await
        .map_err(|error| format!("Model-loading worker failed: {error}"))??;
        Ok(())
    }

    pub fn unload_model(&self) -> Result<(), String> {
        *self
            .engine
            .lock()
            .map_err(|_| "Model engine state is unavailable")? = None;
        if let Ok(mut guard) = self.loaded_name.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.loaded_reasoning.lock() {
            *guard = ReasoningFormat::none();
        }
        Ok(())
    }

    pub fn begin_generation(&self, generation_id: &str) -> Result<Arc<AtomicBool>, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.generations
            .lock()
            .map_err(|_| "Inference state is unavailable")?
            .insert(generation_id.to_owned(), cancel.clone());
        Ok(cancel)
    }

    pub fn end_generation(&self, generation_id: &str) {
        if let Ok(mut generations) = self.generations.lock() {
            generations.remove(generation_id);
        }
    }

    pub fn cancel_generation(&self, generation_id: &str) -> Result<(), String> {
        let generations = self
            .generations
            .lock()
            .map_err(|_| "Inference state is unavailable")?;
        let Some(cancel) = generations.get(generation_id) else {
            return Err("No active generation".into());
        };
        cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Run one completion against the loaded model, invoking `on_delta` for each
    /// decoded piece. Blocking: call from a blocking task.
    // The parameters are all distinct generation controls; grouping them into a
    // struct would only move the same fields elsewhere.
    #[allow(clippy::too_many_arguments)]
    pub fn generate<F>(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
        deadline: Option<Instant>,
        no_progress: Option<Duration>,
        // Output tokens to reserve when trimming the prompt, so the turn can
        // still finish a reasoning block and emit a tool call or an answer.
        min_output: u32,
        cancel: &AtomicBool,
        // Set before the first token: whether the chat template opened a
        // reasoning block, so the caller streams it as reasoning rather than
        // reply text. A reasoning model (Qwen3) appends ` thinking` to the
        // generation prompt, so its output *starts* with the reasoning and has
        // no opening tag of its own.
        reasoning_expected: &AtomicBool,
        mut on_delta: F,
    ) -> Result<Generated, String>
    where
        F: FnMut(&str),
    {
        let guard = self
            .engine
            .lock()
            .map_err(|_| "Model engine state is unavailable")?;
        let loaded = guard.as_ref().ok_or("Load a local model before chatting")?;
        let template = loaded.model.chat_template(None).map_err(|error| {
            format!("The model does not provide a supported chat template: {error}")
        })?;
        // Build the prompt for a candidate message list. Trimming re-templates so
        // the token count is measured on the same text llama.cpp will see.
        let build_prompt = |messages: &[ChatMessage]| -> Result<(String, Vec<llama_cpp_2::token::LlamaToken>), String> {
            let llama_messages = messages
                .iter()
                .filter_map(|message| {
                    let role = match message.role.as_str() {
                        "system" => "system",
                        "assistant" => "assistant",
                        "user" => "user",
                        "tool" => "user",
                        _ => return None,
                    };
                    LlamaChatMessage::new(role.to_owned(), message.content.clone()).ok()
                })
                .collect::<Vec<_>>();
            if llama_messages.is_empty() {
                return Err("Enter a message to start a conversation".into());
            }
            let prompt = loaded
                .model
                .apply_chat_template(&template, &llama_messages, true)
                .map_err(|error| format!("Could not format the chat prompt: {error}"))?;
            let tokens = loaded.model.vocab().tokenize(prompt.as_bytes(), true, true);
            Ok((prompt, tokens))
        };
        let context_size = loaded.context_size;
        // Token-aware budget: keep the system preamble and the newest messages,
        // dropping the oldest non-system ones until the prompt leaves room to
        // answer. Without this a small model that re-decodes a growing history
        // every turn fills the window and the output budget collapses to nothing.
        let mut working = messages.to_vec();
        let (prompt, tokens) = loop {
            let (prompt, tokens) = build_prompt(&working)?;
            if tokens.is_empty() {
                return Err("The model tokenizer returned an empty prompt".into());
            }
            let room = context_size.saturating_sub(tokens.len() as u32);
            if room >= min_output || working.len() <= 1 {
                break (prompt, tokens);
            }
            match working
                .iter()
                .skip(1)
                .position(|message| message.role != "system")
            {
                Some(offset) => {
                    working.remove(offset + 1);
                }
                None => break (prompt, tokens),
            }
        };
        // A template that opened the block leaves the model's first tokens as the
        // reasoning itself, with no opening tag of its own. `loaded.reasoning` is
        // the dialect resolved at load time, so this is model-agnostic.
        reasoning_expected.store(loaded.reasoning.opens_in_prompt(&prompt), Ordering::Relaxed);
        let max_tokens = resolve_max_tokens(max_tokens, tokens.len() as u32, context_size)?;
        // Bound the decode batch independently of the context window. n_batch /
        // n_ubatch drive compute-buffer sizing, so tying them to n_ctx would turn
        // a long context into an enormous allocation. They still have to cover
        // the largest single decode call, or llama.cpp trips a GGML_ASSERT and
        // aborts the process instead of returning an error, hence the chunked
        // prompt evaluation below.
        let batch_size = context_size.min(MAX_DECODE_BATCH);
        let context_params = context_params_with(context_size, batch_size);
        let mut context: LlamaContext =
            loaded
                .model
                .new_context(&loaded.backend, context_params)
                .map_err(|error| format!("Could not create inference context: {error}"))?;
        let mut batch = LlamaBatch::new(batch_size as usize, 1);
        // Evaluate the prompt in chunks that each fit the decode batch. Only the
        // very last token needs logits, because that is the one generation
        // samples from; asking for logits on every chunk would materialise a
        // vocab-sized row per chunk for nothing.
        let last = tokens.len() - 1;
        let mut consumed = 0usize;
        // Row holding the next sample's logits, i32 to match `LlamaBatch::add`.
        // The batch is rebuilt per chunk, so the prompt's final token does not
        // necessarily land on row `last` -- it lands wherever it falls inside the
        // final chunk. Recording the row as we go is the only way to be sure.
        let mut sample_row = 0i32;
        for chunk in tokens.chunks(batch_size as usize) {
            batch.clear();
            for (offset, token) in chunk.iter().enumerate() {
                let index = consumed + offset;
                let wants_logits = index == last;
                if wants_logits {
                    sample_row = offset as i32;
                }
                batch
                    .add(*token, index as i32, &[0], wants_logits)
                    .map_err(|error| format!("Could not prepare prompt: {error}"))?;
            }
            context
                .decode(&mut batch)
                .map_err(|error| format!("Could not evaluate prompt: {error}"))?;
            consumed += chunk.len();
        }
        let mut sampler =
            LlamaSampler::chain_simple([LlamaSampler::temp(0.25), LlamaSampler::dist(42)]);
        let mut decoder = UTF_8.new_decoder();
        let mut text = String::new();
        // The prompt occupies positions 0..prompt_len, so generation continues at
        // `prompt_len` and advances by one per sampled token. i32 because that is
        // what `LlamaBatch::add` takes for a position.
        let prompt_len = tokens.len() as i32;
        // Whether generation ended on purpose (an end token, or a cancellation)
        // rather than by running out of the token budget. A run that ran out is
        // cut off mid-thought, which the caller has to know.
        let mut ended_on_token = false;
        let mut timed_out = false;
        let mut timeout: Option<TimeoutKind> = None;
        let mut generated_tokens = 0usize;
        // Reset per token, so a generation that stalls (a long decode, or a model
        // that stops emitting) is caught by the no-progress limit instead of
        // holding the run at "thinking" indefinitely.
        let mut last_progress = Instant::now();
        for position in (prompt_len..).take(max_tokens as usize) {
            if cancel.load(Ordering::Relaxed) {
                ended_on_token = true;
                break;
            }
            let now = Instant::now();
            if deadline.is_some_and(|deadline| now >= deadline) {
                timed_out = true;
                timeout = Some(TimeoutKind::Deadline);
                break;
            }
            if no_progress.is_some_and(|limit| now.duration_since(last_progress) >= limit) {
                timed_out = true;
                timeout = Some(TimeoutKind::NoProgress);
                break;
            }
            let token = sampler.sample(&context, sample_row);
            sampler.accept(token);
            if loaded.model.vocab().is_eog(token) {
                ended_on_token = true;
                break;
            }
            generated_tokens += 1;
            last_progress = Instant::now();
            let piece = loaded.model.vocab().token_to_piece(token, true, None);
            let mut delta = String::with_capacity(
                decoder
                    .max_utf8_buffer_length(piece.len())
                    .unwrap_or(piece.len() + 4),
            );
            let _ = decoder.decode_to_string(&piece, &mut delta, false);
            if !delta.is_empty() {
                text.push_str(&delta);
                on_delta(&delta);
            }
            batch.clear();
            batch
                .add(token, position, &[0], true)
                .map_err(|error| error.to_string())?;
            context
                .decode(&mut batch)
                .map_err(|error| format!("Token decoding failed: {error}"))?;
            sample_row = 0;
        }
        Ok(Generated {
            text,
            truncated: !ended_on_token,
            tokens: generated_tokens,
            timed_out,
            timeout,
        })
    }
}

/// Which watchdog stopped generation short of the model's end token or the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    /// The overall turn wall-clock elapsed: slow but still progressing.
    Deadline,
    /// No token was produced within the idle window: a real stall.
    NoProgress,
}

/// One local completion.
#[derive(Debug, Clone)]
pub struct Generated {
    pub text: String,
    /// True when generation stopped at the token cap instead of an end token, so
    /// `text` is cut off. A cut tool call is not parseable, and a cut answer is
    /// incomplete; the caller must not present either as a finished result.
    pub truncated: bool,
    /// Tokens actually sampled, so the caller can enforce a whole-run budget.
    pub tokens: usize,
    /// True when the deadline or no-progress limit stopped generation, as opposed
    /// to the model's end token or the token cap.
    pub timed_out: bool,
    /// Which watchdog fired, so the caller can say "slow" instead of always
    /// reporting "stalled".
    pub timeout: Option<TimeoutKind>,
}

fn is_gguf(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
}

/// Per-user data directory holding downloaded GGUFs.
fn models_root() -> PathBuf {
    if let Ok(explicit) = std::env::var("RIGA_MODELS_DIR")
        && !explicit.trim().is_empty()
    {
        let path = PathBuf::from(explicit);
        let _ = fs::create_dir_all(&path);
        return path;
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
        .unwrap_or_else(std::env::temp_dir);
    let path = base.join("riga").join("models");
    let _ = fs::create_dir_all(&path);
    path
}

#[derive(Debug, Clone, Serialize)]
pub struct CuratedModelEntry {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub recommended_context: u32,
    pub max_context: u32,
    pub quant: &'static str,
    pub license_url: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledModel {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub path: String,
    pub size_bytes: u64,
    pub curated: bool,
    pub recommended_context: Option<u32>,
    pub license_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalModelEvent {
    DownloadProgress(DownloadProgress),
    DownloadFinished {
        model_id: String,
        path: String,
    },
    DownloadFailed {
        model_id: String,
        message: String,
    },
    Token {
        generation_id: String,
        delta: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub model_id: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub percent: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Whether the process was built with GPU offload compiled in.
pub fn accelerator_label() -> &'static str {
    if cfg!(feature = "cuda") {
        "CUDA"
    } else {
        "CPU (OpenMP)"
    }
}

#[allow(dead_code)]
fn _now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        CATALOG, Capability, DEFAULT_CONTEXT, LocalBudget, LocalModelRuntime, MAX_BUDGET_TOKENS,
        MAX_CONTEXT, MAX_DECODE_BATCH, MAX_TURNS, MIN_CONTEXT, ReasoningFormat, accelerator_label,
        context_params_with, parse_context_override, parse_reasoning_override, requested_context,
        resolve_context, resolve_max_tokens,
    };

    #[test]
    fn reasoning_format_detects_dialects_from_the_template() {
        // A template that names the tags (Qwen3) resolves to that dialect, so a
        // manually copied GGUF works without a code change.
        assert_eq!(
            ReasoningFormat::detect_from_template("...\u{3c}think\u{3e}...\u{3c}/think\u{3e}..."),
            ReasoningFormat::think()
        );
        // A template with no reasoning tags means the model has no channel.
        assert!(!ReasoningFormat::detect_from_template("<|im_start|>assistant\n").enabled());
    }

    #[test]
    fn reasoning_format_strips_only_its_own_dialect() {
        let think = ReasoningFormat::think();
        assert_eq!(
            think.strip("\u{3c}think\u{3e}\nwhy\n\u{3c}/think\u{3e}\nanswer"),
            "answer"
        );
        // A model with no reasoning channel strips nothing.
        assert_eq!(
            ReasoningFormat::none().strip("\u{3c}think\u{3e}\nanswer"),
            "\u{3c}think\u{3e}\nanswer"
        );
    }

    #[test]
    fn reasoning_override_forces_the_dialect_on_or_off() {
        assert!(!parse_reasoning_override(Some("off"), ReasoningFormat::think()).enabled());
        assert!(parse_reasoning_override(Some("on"), ReasoningFormat::none()).enabled());
        assert_eq!(
            parse_reasoning_override(Some("auto"), ReasoningFormat::think()),
            ReasoningFormat::think()
        );
    }

    #[test]
    fn context_defaults_to_the_safe_window_unless_overridden() {
        // A curated model's larger window is capped at the default: fitting a
        // 32k KV cache on a consumer GPU offloads fewer layers and decode crawls.
        assert_eq!(requested_context(Some(32_768), None), DEFAULT_CONTEXT);
        // A curated window smaller than the default is respected.
        assert_eq!(requested_context(Some(4_096), None), 4_096);
        // Non-curated model: the safe default.
        assert_eq!(requested_context(None, None), DEFAULT_CONTEXT);
        // An explicit override wins, and resolve_context still clamps it to the
        // GGUF's trained window.
        assert_eq!(requested_context(Some(32_768), Some("4096")), 4_096);
        assert_eq!(
            resolve_context(131_072, requested_context(Some(32_768), Some("16384"))),
            16_384
        );
        // Junk or zero falls back rather than panicking in `clamp`.
        assert_eq!(
            requested_context(Some(32_768), Some("nonsense")),
            DEFAULT_CONTEXT
        );
        assert_eq!(requested_context(Some(32_768), Some("0")), DEFAULT_CONTEXT);
        // An override below the floor is clamped up, not accepted as-is.
        assert_eq!(
            resolve_context(131_072, requested_context(None, Some("1"))),
            MIN_CONTEXT
        );
    }

    #[test]
    fn curated_catalog_has_unique_fully_hashed_entries() {
        assert_eq!(CATALOG.len(), 4);
        let mut ids = std::collections::HashSet::new();
        let mut file_names = std::collections::HashSet::new();
        for model in CATALOG {
            assert!(ids.insert(model.id));
            assert!(file_names.insert(model.file_name));
            assert!(
                model.sha256.len() == 64 && model.sha256.chars().all(|c| c.is_ascii_hexdigit())
            );
            assert!(model.download_url.starts_with("https://"));
            assert!(model.file_name.ends_with(".gguf"));
        }
    }

    #[test]
    fn every_curated_window_is_real_and_never_exceeds_our_ceiling() {
        // These came from the `*.context_length` key of the exact GGUF each entry
        // downloads. A regression means a download URL was repointed at a
        // different quantization, which invalidates the number.
        let expected = [
            ("qwen3-4b-instruct", 32_768),
            ("qwen2.5-coder-7b-instruct", 32_768),
            ("phi-4-mini-instruct", 131_072),
            ("hermes-3-llama-3.1-8b", 131_072),
        ];
        assert_eq!(
            CATALOG
                .iter()
                .map(|m| (m.id, m.max_context))
                .collect::<Vec<_>>(),
            expected
        );
        // The 128k windows are truncated to our ceiling by `resolve_context`.
        assert_eq!(resolve_context(8_192, 8_192), 8_192);
        assert_eq!(resolve_context(131_072, 131_072), MAX_CONTEXT);
        assert_eq!(resolve_context(32_768, 32_768), 32_768);
    }

    #[test]
    fn context_never_exceeds_what_the_gguf_was_trained_for() {
        // The GGUF header is authoritative, so a curated entry that disagrees
        // with the file on disk must lose.
        assert_eq!(resolve_context(2_048, 131_072), 2_048);
        assert_eq!(resolve_context(0, 131_072), MIN_CONTEXT);
        // A tiny window still leaves room for the floor.
        assert_eq!(resolve_context(512, 131_072), MIN_CONTEXT);
        // Nothing is ever handed back above the ceiling, whatever is asked for.
        for trained in [1_024, 8_192, 32_768, 131_072] {
            for wanted in [0, 1, 4_096, 1_000_000] {
                let resolved = resolve_context(trained, wanted);
                assert!((MIN_CONTEXT..=MAX_CONTEXT).contains(&resolved));
                assert!(resolved <= trained.clamp(MIN_CONTEXT, u32::MAX));
            }
        }
    }

    #[test]
    fn max_tokens_shrink_to_the_room_the_prompt_leaves() {
        // The common case: a short prompt leaves far more than we asked for.
        assert_eq!(resolve_max_tokens(512, 900, 32_768).unwrap(), 512);
        // A long prompt shrinks the completion instead of failing the request.
        assert_eq!(resolve_max_tokens(512, 32_500, 32_768).unwrap(), 268);
        // A stale client asking for more than the context can hold is bounded.
        assert_eq!(resolve_max_tokens(999_999, 10, 8_192).unwrap(), 8_182);
        // A zero request still yields a usable, non-empty completion.
        assert_eq!(resolve_max_tokens(0, 10, 8_192).unwrap(), 1);
        // Only a prompt that fills the context outright is an error.
        let err = resolve_max_tokens(512, 8_192, 8_192).unwrap_err();
        assert!(
            err.contains("8192-token context"),
            "unhelpful message: {err}"
        );
        assert!(resolve_max_tokens(512, 9_000, 8_192).is_err());
    }

    #[test]
    fn decode_batch_stays_well_inside_every_context_we_hand_out() {
        // Every window resolve_context can return must still admit a decode
        // batch that holds at least one token and never exceeds the context.
        for trained in [0, 512, MIN_CONTEXT, DEFAULT_CONTEXT, MAX_CONTEXT, 131_072] {
            let context = resolve_context(trained, MAX_CONTEXT);
            let batch_size = context.min(MAX_DECODE_BATCH);
            assert!(batch_size >= 1, "context {context} leaves no decode room");
            assert!(
                batch_size <= context,
                "batch {batch_size} exceeds {context}"
            );
        }
    }

    #[test]
    fn accelerator_label_names_the_built_in_backend() {
        let label = accelerator_label();
        assert!(
            label == "CPU (OpenMP)" || label == "CUDA",
            "unexpected: {label}"
        );
    }

    #[test]
    fn an_unknown_model_id_is_rejected_before_a_download_is_accepted() {
        // The request returns as soon as the transfer is spawned, so an id that
        // only fails inside the task would be answered "started" and the picker
        // would wait on a progress bar that never moves.
        let runtime = LocalModelRuntime::default();
        for bad in [
            "not-a-real-id",
            "",
            "PHI-4-MINI-INSTRUCT",
            "../../etc/passwd",
        ] {
            let blocker = runtime
                .download_blocker(bad)
                .unwrap_or_else(|| panic!("{bad:?} was accepted for download"));
            assert!(
                blocker.contains("Unknown curated model"),
                "unhelpful blocker for {bad:?}: {blocker}"
            );
        }
    }

    #[test]
    fn every_curated_id_is_accepted_and_nothing_else_is() {
        // Guards both directions: the guard must not reject a real entry, or the
        // download button in the UI would be dead for every model.
        let runtime = LocalModelRuntime::default();
        for model in CATALOG {
            assert_eq!(
                runtime.download_blocker(model.id),
                None,
                "curated model {} was refused",
                model.id
            );
        }
    }

    #[test]
    fn capability_tiers_are_ordered_and_parse() {
        let compact = Capability::Compact.budget();
        let middle = Capability::Middle.budget();
        let large = Capability::Large.budget();
        assert!(compact.max_tokens < middle.max_tokens);
        assert!(middle.max_tokens < large.max_tokens);
        assert!(compact.max_turns < middle.max_turns);
        assert!(middle.max_turns < large.max_turns);
        assert!(compact.turn_seconds < middle.turn_seconds);
        assert!(middle.turn_seconds < large.turn_seconds);
        // Middle is the default, so behaviour is unchanged unless asked.
        assert_eq!(LocalBudget::default(), middle);
        assert_eq!(Capability::parse("Compact"), Some(Capability::Compact));
        assert_eq!(Capability::parse(" medium "), Some(Capability::Middle));
        assert_eq!(Capability::parse("gigantic"), None);
    }

    #[test]
    fn enlarge_and_shrink_stay_within_bounds() {
        let compact = Capability::Compact.budget();
        assert!(compact.enlarged().max_tokens > compact.max_tokens);
        let mut huge = compact;
        for _ in 0..20 {
            huge = huge.enlarged();
        }
        assert_eq!(huge.max_tokens, MAX_BUDGET_TOKENS);
        assert_eq!(huge.max_turns, MAX_TURNS);
        // Shrinking from the floor cannot go lower.
        assert_eq!(compact.shrunk().max_tokens, compact.max_tokens);
        assert_eq!(compact.shrunk().max_turns, compact.max_turns);
    }

    #[test]
    fn budget_apply_honours_tier_enlarge_and_explicit_overrides() {
        let mut budget = LocalBudget::default();
        let summary = budget
            .apply(&serde_json::json!({"tier": "compact"}))
            .expect("tier applies");
        assert_eq!(budget, Capability::Compact.budget());
        assert!(summary.contains("max_tokens=2048"), "{summary}");

        budget
            .apply(&serde_json::json!({"enlarge": true}))
            .expect("enlarge applies");
        assert!(budget.max_tokens > Capability::Compact.budget().max_tokens);

        // An explicit value below the floor is clamped up, not accepted as-is.
        budget
            .apply(&serde_json::json!({"max_tokens": 10}))
            .expect("override applies");
        assert_eq!(budget.max_tokens, Capability::Compact.budget().max_tokens);

        let error = budget
            .apply(&serde_json::json!({"tier": "gigantic"}))
            .expect_err("unknown tier rejected");
        assert!(error.contains("unknown capability tier"), "{error}");
    }

    #[test]
    fn list_installed_recognizes_curated_and_manually_copied_models() {
        // A model the user dropped into the models directory must still show up,
        // flagged as not curated, alongside a curated download.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("my-custom-model.gguf"), b"x").unwrap();
        let curated = CATALOG[0].file_name;
        std::fs::write(dir.path().join(curated), b"x").unwrap();

        let installed = LocalModelRuntime::list_models_in(dir.path()).unwrap();

        assert_eq!(installed.len(), 2);
        let manual = installed
            .iter()
            .find(|model| model.file_name == "my-custom-model.gguf")
            .expect("manual model is listed");
        assert!(!manual.curated, "a manually copied model is not curated");
        assert_eq!(manual.id, "my-custom-model.gguf");
        assert_eq!(manual.name, "my-custom-model.gguf");
        let known = installed
            .iter()
            .find(|model| model.file_name == curated)
            .expect("curated model is listed");
        assert!(known.curated);
        assert_eq!(known.id, CATALOG[0].id);
    }

    #[test]
    fn context_override_parsing_rejects_junk_and_zero() {
        assert_eq!(parse_context_override(Some("16384")), Some(16_384));
        assert_eq!(parse_context_override(Some(" 32768 ")), Some(32_768));
        assert_eq!(parse_context_override(Some("0")), None);
        assert_eq!(parse_context_override(Some("junk")), None);
        assert_eq!(parse_context_override(None), None);
    }

    #[test]
    fn context_params_carry_the_requested_window() {
        assert_eq!(
            context_params_with(16_384, 2_048)
                .n_ctx()
                .map(std::num::NonZeroU32::get),
            Some(16_384)
        );
    }

    #[test]
    fn context_params_cap_the_ubatch_to_bound_the_compute_buffer() {
        let params = context_params_with(16_384, 2_048);
        assert_eq!(params.n_batch(), 2_048);
        assert_eq!(params.n_ubatch(), 512);
    }
}

//! Query embedding with llama.cpp (Qwen3-Embedding-0.6B GGUF), CPU only.
//!
//! Must match `ingest/embed.py`: the tokenizer output ends in `<|endoftext|>`, last-token
//! pooling, L2 normalisation. The query instruction prefix comes from the pack manifest.

use anyhow::{Context, Result};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const MAX_TOKENS: usize = 512;

/// Holds one llama context for the process lifetime (creating one costs ~45 ms per query).
/// The backend and model are leaked to `'static` so the context can borrow them; an
/// embedder lives as long as the app or server anyway.
pub struct Embedder {
    model: &'static LlamaModel,
    ctx: Mutex<LlamaContext<'static>>,
}

// llama.cpp contexts are not thread-safe; every use goes through the Mutex.
unsafe impl Send for Embedder {}
unsafe impl Sync for Embedder {}

/// llama.cpp's backend can be initialised once per process; the app restarts its server
/// (and so reloads the model) after downloads, so it is shared (also by sp-rag).
static BACKEND: OnceLock<&'static LlamaBackend> = OnceLock::new();

pub fn backend() -> Result<&'static LlamaBackend> {
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    let mut b = LlamaBackend::init()?;
    b.void_logs();
    Ok(BACKEND.get_or_init(|| Box::leak(Box::new(b))))
}

impl Embedder {
    pub fn load(gguf: &Path, threads: usize) -> Result<Self> {
        let backend = backend()?;
        let model = LlamaModel::load_from_file(backend, gguf, &LlamaModelParams::default())
            .with_context(|| format!("loading {}", gguf.display()))?;
        let model: &'static LlamaModel = Box::leak(Box::new(model));
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(MAX_TOKENS as u32))
            .with_n_batch(MAX_TOKENS as u32)
            .with_n_ubatch(MAX_TOKENS as u32)
            .with_n_threads(threads as i32)
            .with_n_threads_batch(threads as i32)
            .with_embeddings(true)
            .with_pooling_type(LlamaPoolingType::Last);
        let ctx = model.new_context(backend, params)?;
        Ok(Embedder { model, ctx: Mutex::new(ctx) })
    }

    pub fn dims(&self) -> usize {
        self.model.n_embd() as usize
    }

    /// Full-dimension, L2-normalised embedding of `text` (prefix already applied).
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let vocab = self.model.vocab();
        let mut tokens = vocab.tokenize(text.as_bytes(), false, false);
        tokens.truncate(MAX_TOKENS - 1);
        if tokens.last() != Some(&vocab.eos()) {
            tokens.push(vocab.eos());
        }
        let mut ctx = self.ctx.lock().unwrap();
        ctx.clear_kv_cache();
        let mut batch = LlamaBatch::new(MAX_TOKENS, 1);
        batch.add_sequence(&tokens, 0, false)?;
        ctx.decode(&mut batch)?;
        let v = ctx.embeddings_seq_ith(0)?;
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        Ok(v.iter().map(|x| x / n).collect())
    }
}

/// Default thread count: `SP_THREADS`, else ~80% of cores on big machines (all cores up to 8).
/// On the GB10 (20 cores) 16 threads gives ~95 ms per query; 20 is slower (contention).
pub fn default_threads() -> usize {
    std::env::var("SP_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        if n > 8 { n * 4 / 5 } else { n }
    })
}

//! [`Embedder`] — lazy-loaded embedding inference (REQ-401, REQ-402 in
//! `.specs/features/vector-engine/spec.md`). Wraps an ONNX sentence-
//! embedding model (e.g. `all-MiniLM-L6-v2`, quantized INT8, ~23MB) run
//! in-process via `ort`, plus its `tokenizers`-format tokenizer.
//!
//! Model/tokenizer paths are caller-supplied configuration, never bundled
//! or downloaded by this crate itself (see
//! `.specs/features/vector-engine/tasks.md` → T-406) — a future CLI
//! command is the right place to automate fetching them.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    #[error("embedding model not available at {0}")]
    ModelNotAvailable(PathBuf),
    #[error("inference error: {0}")]
    Inference(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

struct Runtime {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

/// Holds the *configuration* (where the model/tokenizer live) and a lazy
/// slot for the loaded runtime — nothing is loaded until `embed()` is
/// first called (REQ-401). Constructing an `Embedder` never touches disk.
pub struct Embedder {
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    // `Result` cached too, so a load failure isn't retried (and re-erred
    // identically) on every call -- matches OnceLock's "exactly once"
    // spirit for the failure case as well as the success case.
    runtime: OnceLock<Result<Runtime, String>>,
}

impl Embedder {
    pub fn new(model_path: impl Into<PathBuf>, tokenizer_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            runtime: OnceLock::new(),
        }
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    pub fn tokenizer_path(&self) -> &Path {
        &self.tokenizer_path
    }

    fn runtime(&self) -> Result<&Runtime, VectorError> {
        let loaded = self.runtime.get_or_init(|| {
            Self::load(&self.model_path, &self.tokenizer_path).map_err(|e| e.to_string())
        });
        loaded.as_ref().map_err(|e| VectorError::Inference(e.clone()))
    }

    fn load(model_path: &Path, tokenizer_path: &Path) -> Result<Runtime, VectorError> {
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VectorError::Inference(format!("tokenizer: {e}")))?;
        let session = Session::builder()
            .map_err(|e| VectorError::Inference(format!("session builder: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| VectorError::Inference(format!("model load: {e}")))?;
        Ok(Runtime {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    /// Embed `text` into a mean-pooled, L2-normalized vector (dimension
    /// depends on the model — 384 for `all-MiniLM-L6-v2`/
    /// `bge-small-en-v1.5`). Returns [`VectorError::ModelNotAvailable`] --
    /// cleanly, no panic, no network access -- if the configured model or
    /// tokenizer file isn't on disk. The `ort::Session`/tokenizer are
    /// loaded exactly once, on the first call that gets past that check
    /// (REQ-401).
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, VectorError> {
        if !self.model_path.exists() {
            return Err(VectorError::ModelNotAvailable(self.model_path.clone()));
        }
        if !self.tokenizer_path.exists() {
            return Err(VectorError::ModelNotAvailable(self.tokenizer_path.clone()));
        }
        let runtime = self.runtime()?;

        let encoding = runtime
            .tokenizer
            .encode(text, true)
            .map_err(|e| VectorError::Inference(format!("tokenize: {e}")))?;
        let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| i64::from(x)).collect();
        let mask: Vec<i64> = encoding.get_attention_mask().iter().map(|&x| i64::from(x)).collect();
        let type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&x| i64::from(x)).collect();
        let seq_len = ids.len();
        let shape = vec![1i64, seq_len as i64];

        let input_ids = Tensor::<i64>::from_array((shape.clone(), ids))
            .map_err(|e| VectorError::Inference(format!("input_ids tensor: {e}")))?;
        let attention_mask = Tensor::<i64>::from_array((shape.clone(), mask.clone()))
            .map_err(|e| VectorError::Inference(format!("attention_mask tensor: {e}")))?;
        let token_type_ids = Tensor::<i64>::from_array((shape, type_ids))
            .map_err(|e| VectorError::Inference(format!("token_type_ids tensor: {e}")))?;

        let mut session = runtime.session.lock().unwrap();
        let outputs = session
            .run(ort::inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
                "token_type_ids" => token_type_ids,
            ])
            .map_err(|e| VectorError::Inference(format!("run: {e}")))?;

        let (out_shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| VectorError::Inference(format!("extract output: {e}")))?;
        let seq = out_shape[1] as usize;
        let hidden_dim = out_shape[2] as usize;

        // Mean pooling over non-padding tokens (attention_mask == 1),
        // matching sentence-transformers' standard pooling strategy.
        let mut pooled = vec![0f32; hidden_dim];
        let mut count = 0f32;
        for t in 0..seq {
            if mask[t] == 1 {
                for h in 0..hidden_dim {
                    pooled[h] += data[t * hidden_dim + h];
                }
                count += 1.0;
            }
        }
        if count > 0.0 {
            for v in &mut pooled {
                *v /= count;
            }
        }

        let norm: f32 = pooled.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut pooled {
                *v /= norm;
            }
        }
        Ok(pooled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_reports_model_not_available_without_touching_the_network() {
        let embedder = Embedder::new("/nonexistent/model.onnx", "/nonexistent/tokenizer.json");

        let result = embedder.embed("hello world");

        assert!(matches!(result, Err(VectorError::ModelNotAvailable(_))));
    }

    #[test]
    fn constructing_an_embedder_never_touches_disk() {
        // If `new()` did any I/O, this nonexistent-but-plausible-looking
        // path would already have surfaced an error; it doesn't even try.
        let _embedder = Embedder::new("/definitely/does/not/exist.onnx", "/definitely/does/not/exist.json");
    }

    fn model_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".models")
    }

    /// Ignored by default -- only runs when the real model is present on
    /// disk (T-406, downloaded outside version control). Run explicitly
    /// with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn real_inference_produces_a_deterministic_384_dim_vector() {
        let embedder = Embedder::new(
            model_dir().join("model_quantized.onnx"),
            model_dir().join("tokenizer.json"),
        );

        let first = embedder.embed("hello world").unwrap();
        let second = embedder.embed("hello world").unwrap();

        assert_eq!(first.len(), 384);
        assert_eq!(first, second, "embedding the same text twice must be deterministic");

        let norm: f32 = first.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "output should be L2-normalized, got norm {norm}");
    }

    #[test]
    #[ignore]
    fn real_inference_ranks_similar_sentences_closer_than_dissimilar_ones() {
        let embedder = Embedder::new(
            model_dir().join("model_quantized.onnx"),
            model_dir().join("tokenizer.json"),
        );

        let a = embedder.embed("The cat sat on the mat").unwrap();
        let b = embedder.embed("A cat was sitting on a mat").unwrap();
        let c = embedder.embed("Quarterly revenue exceeded analyst expectations").unwrap();

        let cosine = |x: &[f32], y: &[f32]| -> f32 { x.iter().zip(y).map(|(p, q)| p * q).sum() };
        let sim_ab = cosine(&a, &b);
        let sim_ac = cosine(&a, &c);

        assert!(
            sim_ab > sim_ac,
            "semantically similar sentences ({sim_ab}) must score higher than unrelated ones ({sim_ac})"
        );
    }
}

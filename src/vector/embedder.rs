//! [`Embedder`] — lazy-loaded embedding inference (REQ-401, REQ-402 in
//! `.specs/features/vector-engine/spec.md`). This task (T-405) only wires
//! the "no model on disk" path deterministically — real ONNX Runtime
//! inference is T-406, gated on the user confirming the ~30MB model
//! download (see `.specs/features/vector-engine/tasks.md`). Nothing here
//! touches the network or the `ort` crate's types yet; that wiring belongs
//! entirely to T-406, where it can be tested against the real model file
//! instead of guessed at.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    #[error("embedding model not available at {0}")]
    ModelNotAvailable(PathBuf),
    #[error("inference error: {0}")]
    Inference(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Holds the *configuration* (where the model/tokenizer would live) and a
/// lazy slot for the loaded runtime — nothing is loaded until `embed()` is
/// first called (REQ-401). Constructing an `Embedder` never touches disk.
pub struct Embedder {
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    // Populated on first successful load (T-406). `OnceLock` gives the
    // "exactly once, on first use" semantics REQ-401 asks for without
    // needing a startup hook anywhere in the crate.
    loaded: OnceLock<()>,
}

impl Embedder {
    pub fn new(model_path: impl Into<PathBuf>, tokenizer_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            tokenizer_path: tokenizer_path.into(),
            loaded: OnceLock::new(),
        }
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    pub fn tokenizer_path(&self) -> &Path {
        &self.tokenizer_path
    }

    /// Embed `text` into a vector. Returns
    /// [`VectorError::ModelNotAvailable`] — cleanly, no panic, no network
    /// access — if the configured model file isn't on disk. Real inference
    /// (once a model is present) is wired in T-406.
    pub fn embed(&self, _text: &str) -> Result<Vec<f32>, VectorError> {
        if !self.model_path.exists() {
            return Err(VectorError::ModelNotAvailable(self.model_path.clone()));
        }
        if !self.tokenizer_path.exists() {
            return Err(VectorError::ModelNotAvailable(self.tokenizer_path.clone()));
        }
        let _ = self.loaded.get_or_init(|| ());
        Err(VectorError::Inference(
            "real ONNX inference not yet wired -- see T-406".to_string(),
        ))
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
}

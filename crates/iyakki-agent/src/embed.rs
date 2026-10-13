//! Text embeddings. The default needs no model or network (hashed bag-of-words); when an embedding
//! model is chosen in Settings, [`crate::services`] supplies real semantic vectors instead. Vectors from different embedders are never
//! mixed: each namespace records the embedder id it was built with.

use iyakki_mem::{Embedder as _, HashEmbedder};

pub trait TextEmbedder: Send + Sync {
    /// Stable identifier stored with each namespace, e.g. `hash-256` or `remote:nomic-embed-text`.
    fn id(&self) -> String;
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

/// Dependency-free fallback: lexical-semantic hashing. Good for keyword-ish recall, offline-safe.
pub struct HashEmbed {
    inner: HashEmbedder,
}

impl Default for HashEmbed {
    fn default() -> Self {
        Self { inner: HashEmbedder { dim: 256 } }
    }
}

impl TextEmbedder for HashEmbed {
    fn id(&self) -> String {
        format!("hash-{}", self.inner.dim)
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts.iter().map(|t| self.inner.embed(t)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_embedder_is_deterministic_and_normalised() {
        let e = HashEmbed::default();
        let v = e.embed(&["rust wasm sandbox".into(), "rust wasm sandbox".into()]).unwrap();
        assert_eq!(v[0], v[1]);
        let n: f32 = v[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-4);
        assert_eq!(e.id(), "hash-256");
    }
}

//! The embedder interface (moved down from `iyakki-memx`, which re-exports it): text in, one vector per text out.

pub trait TextEmbedder: Send + Sync {
    /// Stable identifier stored with each namespace, e.g. `hash-256` or `remote:nomic-embed-text`.
    fn id(&self) -> String;
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

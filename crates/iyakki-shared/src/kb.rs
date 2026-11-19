//! Types moved out of `iyakki_agent::kb` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Hit {
    pub ns: String,
    pub doc_id: String,
    pub title: String,
    pub source: String,
    pub chunk_idx: u32,
    pub text: String,
    /// Final fused score (higher is better; only meaningful relative to other hits of one query).
    pub score: f32,
    pub bm25: f32,
    pub vector: f32,
}

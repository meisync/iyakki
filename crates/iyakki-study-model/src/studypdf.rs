//! What a study PDF export covers (moved out of `iyakki_agent::studypdf`, F-SPLIT-03; alpha.8 E3: now in the leaf crate `iyakki-study-model`).


/// What to export.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Item {
    Plan(String),
    /// `key`: put the answer key on a page of its own after the questions.
    Quiz { id: String, key: bool },
    /// Day `n` (1-based) of a plan.
    Lesson { plan: String, n: usize },
}

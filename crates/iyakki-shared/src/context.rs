//! Types moved out of `iyakki_agent::context` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ContextOptions {
    /// Most recent non-system messages sent verbatim.
    pub window: usize,
    pub memory_k: usize,
    pub docs_k: usize,
    pub use_docs: bool,
    pub use_skills: bool,
    /// Skills that are always included, before the one chosen by similarity (at most three pinned): the stack's language skill, the k3s practices.
    pub pinned_skills: Vec<String>,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self { window: 12, memory_k: 3, docs_k: 4, use_docs: false, use_skills: true, pinned_skills: Vec::new() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextInfo {
    pub skills: Vec<String>,
    pub recalled: usize,
    pub doc_sources: Vec<String>,
}

impl ContextInfo {
    /// Short human summary for the status line ("" when nothing was added).
    pub fn summary(&self) -> String {
        let mut p = Vec::new();
        if !self.skills.is_empty() {
            p.push(format!("skills: {}", self.skills.join(", ")));
        }
        if self.recalled > 0 {
            p.push(format!("recalled {} earlier turn{}", self.recalled, if self.recalled == 1 { "" } else { "s" }));
        }
        if !self.doc_sources.is_empty() {
            p.push(format!("{} document passage{}", self.doc_sources.len(), if self.doc_sources.len() == 1 { "" } else { "s" }));
        }
        p.join(" · ")
    }
}

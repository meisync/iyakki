//! Types moved out of `iyakki_agent::aiprofile` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What the profile says about one tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vis {
    Show,
    /// Shown, and the reason is that the cluster has the GPU this machine lacks ("your cluster 'x' has 2 x ...").
    ShownBecause(String),
    /// Shown with a star and the reason.
    Recommended(String),
    /// Shown, with the reason it is not a good idea here.
    NotRecommended(String),
    /// Not shown (unless "Show all AI tools" is on), with the reason.
    Hidden(String),
}

impl Vis {
    pub fn shown(&self) -> bool {
        !matches!(self, Vis::Hidden(_))
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Vis::Show => None,
            Vis::ShownBecause(r) | Vis::Recommended(r) | Vis::NotRecommended(r) | Vis::Hidden(r) => Some(r),
        }
    }
}

/// The least usable memory (GB) of a GPU or Apple chip for local language models (the agent keeps the same number).
pub const MIN_LLM_GB: f64 = 6.0;

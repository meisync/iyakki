//! Types moved out of `iyakki_agent::studypdf` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use crate::study::Plan;
use crate::study::Quiz;
use crate::studylesson::Lesson;

/// What to export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Plan(String),
    /// `key`: put the answer key on a page of its own after the questions.
    Quiz { id: String, key: bool },
    /// Day `n` (1-based) of a plan.
    Lesson { plan: String, n: usize },
}

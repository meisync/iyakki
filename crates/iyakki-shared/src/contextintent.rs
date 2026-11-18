//! Types moved out of `iyakki_agent::contextintent` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    Add { text: String },
    Forget { query: String },
    Update { query: String, text: String },
    Pin { query: Option<String>, pin: bool },
    List,
}

/// An intent and what is left of the message after it (a task for the model, or nothing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub intent: Intent,
    pub rest: String,
}

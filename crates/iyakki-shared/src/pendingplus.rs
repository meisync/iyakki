//! Types moved out of `iyakki_agent::pendingplus` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.


/// One row of the Settings page: key, label, shown value and a hint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
    pub hint: &'static str,
}

//! Types moved out of `iyakki_agent::cli` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Clear,
    PowerOff,
    Reboot,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CliOut {
    pub text: String,
    pub action: Action,
}


//! Types moved out of `iyakki_agent::ollama` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OllamaState {
    Stopped,
    Starting,
    Running {
        /// Started by someone else: used, never stopped from here.
        external: bool,
        version: String,
    },
    Stopping,
    Failed(String),
}

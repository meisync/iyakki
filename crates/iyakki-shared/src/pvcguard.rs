//! Types moved out of `iyakki_agent::pvcguard` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What a `kubectl` call answered.
#[derive(Clone, Debug, Default)]
pub struct KOut {
    pub ok: bool,
    pub stdout: String,
    /// Standard output and error together (for messages).
    pub text: String,
}

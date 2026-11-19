//! Types moved out of `iyakki_agent::history` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// One workspace's history, for Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceMemory {
    pub id: String,
    pub name: String,
    pub entries: usize,
    pub chats: usize,
}

//! Types moved out of `iyakki_agent::cluster` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClusterState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed(String),
}

impl ClusterState {
    pub fn label(&self) -> String {
        match self {
            ClusterState::Stopped => "stopped".into(),
            ClusterState::Starting => "starting (the first start takes a minute or two)".into(),
            ClusterState::Running => "running".into(),
            ClusterState::Stopping => "stopping".into(),
            ClusterState::Failed(e) => format!("failed: {e}"),
        }
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxResult {
    pub ok: bool,
    pub output: String,
    pub seconds: u64,
}

//! Types moved out of `iyakki_agent::camfw` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What the machine shows after the driver was loaded again.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Seen {
    /// Video devices of the camera (`/dev/video0`).
    pub devices: Vec<String>,
}

/// The plain result of a run, also written to `last.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Outcome {
    pub at: i64,
    pub installed: bool,
    pub firmware_version: String,
    pub sensor_files: usize,
    pub seen: Seen,
    pub headline: String,
    pub lines: Vec<String>,
    pub kernel: Vec<String>,
}

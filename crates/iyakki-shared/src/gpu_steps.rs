//! Types moved out of `iyakki_agent::gpu_steps` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// Where Lemonade runs its models: on the GPU (the default, when a GPU step is installed) or on the CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compute {
    Gpu,
    Cpu,
}

impl Compute {
    pub fn word(self) -> &'static str {
        match self {
            Compute::Gpu => "gpu",
            Compute::Cpu => "cpu",
        }
    }
    pub fn parse(s: &str) -> Option<Compute> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gpu" => Some(Compute::Gpu),
            "cpu" => Some(Compute::Cpu),
            _ => None,
        }
    }
}

use std::path::Path;

/// The slow part of the report: the kernel log and the pods that use the GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Background {
    pub kernel_log: String,
    pub pods: Vec<crate::gpu::GpuPod>,
    /// Why the pods could not be listed (no cluster, not Ready ...), empty when they could.
    pub pods_note: String,
}

static BACKGROUND: std::sync::Mutex<Option<Background>> = std::sync::Mutex::new(None);


/// The answer of [`refresh_async`], once it is there.
pub fn take_background() -> Option<Background> {
    BACKGROUND.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Merge the slow part into the report.
pub fn merge_background(r: &mut crate::gpu::Report, b: &Background, root: &Path) {
    r.pods = b.pods.clone();
    r.pods_note = b.pods_note.clone();
    r.driver = crate::gpu::driver_state(root, &r.gpus, &b.kernel_log);
}


/// Put the slow part of the report in place (or clear it).
pub fn set_background(b: Option<Background>) {
    *BACKGROUND.lock().unwrap_or_else(|e| e.into_inner()) = b;
}

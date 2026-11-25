//! The hardware facts the Setup tab reads (alpha.10 AP3, D-AP3-02 GPU and firmware groups, D-AP3-03 (2) and (3)): the graphics
//! cards, the camera and the Mac firmware outcomes, the firmware plan, the desktop-window test and the voice pack helpers. Plain answers
//! only; the real port is `AgentSetupHw` in `crates/iyakki-apps/src/apps/store_port.rs`.

use std::path::Path;

/// The graphics devices of this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuFacts {
    /// Graphics devices with a render node (virtual ones left out).
    pub count: usize,
    /// One of them is NVIDIA's.
    pub nvidia: bool,
    /// One of them can run the AMD compute stack.
    pub amd_rocm: bool,
}

/// The last result of a hardware install, as the card shows it: the headline, and whether the install went through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FwNote {
    pub headline: String,
    pub installed: bool,
}

pub trait SetupHwPort: Send + Sync {
    fn gpu_facts(&self) -> GpuFacts;
    /// The camera of the MacBook is on the bus under `sys_root`.
    fn camera_in(&self, sys_root: &Path) -> bool;
    /// The firmware plan of the hardware item is empty (nothing to install).
    fn firmware_plan_empty(&self, data: &Path) -> bool;
    /// This computer is a desktop window of another computer (IYAKKI never installs a cluster there).
    fn window_session(&self) -> bool;
    fn camera_outcome(&self, data: &Path) -> Option<FwNote>;
    /// Files of the camera install the person may still delete.
    fn camera_leftovers(&self, data: &Path) -> u64;
    fn camera_delete_leftovers(&self, data: &Path) -> u64;
    fn mac_outcome(&self, data: &Path) -> Option<FwNote>;
    /// The one-line state of the Mac firmware's download cache (empty when none).
    fn mac_card_status(&self, data: &Path) -> String;
    /// The lines of the voice pack's review (the speech server's own words).
    fn voicepack_review_lines(&self, lemonade_there: bool) -> Vec<String>;
    fn voicepack_delete_model(&self, model: &str) -> Result<(), String>;
}

/// Canned answers (tests): no graphics, no camera, no outcomes, an empty review.
#[derive(Debug, Default)]
pub struct FakeSetupHw;

impl SetupHwPort for FakeSetupHw {
    fn gpu_facts(&self) -> GpuFacts {
        GpuFacts::default()
    }
    fn camera_in(&self, _sys_root: &Path) -> bool {
        false
    }
    fn firmware_plan_empty(&self, _data: &Path) -> bool {
        true
    }
    fn window_session(&self) -> bool {
        false
    }
    fn camera_outcome(&self, _data: &Path) -> Option<FwNote> {
        None
    }
    fn camera_leftovers(&self, _data: &Path) -> u64 {
        0
    }
    fn camera_delete_leftovers(&self, _data: &Path) -> u64 {
        0
    }
    fn mac_outcome(&self, _data: &Path) -> Option<FwNote> {
        None
    }
    fn mac_card_status(&self, _data: &Path) -> String {
        String::new()
    }
    fn voicepack_review_lines(&self, _lemonade_there: bool) -> Vec<String> {
        Vec::new()
    }
    fn voicepack_delete_model(&self, _model: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_has_no_hardware_and_nothing_to_delete() {
        let f = FakeSetupHw;
        assert_eq!(f.gpu_facts(), GpuFacts::default());
        assert!(!f.camera_in(Path::new("/")) && f.firmware_plan_empty(Path::new("/")) && !f.window_session());
        assert_eq!(f.camera_delete_leftovers(Path::new("/")), 0);
        assert!(f.voicepack_delete_model("m").is_ok());
    }
}

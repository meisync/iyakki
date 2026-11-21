//! The fake [`Installer`]: canned disks and a canned source, a scripted progress, every plan it was asked to write recorded, nothing written.

use std::sync::Mutex;
use std::time::Duration;

use iyakki_core_api::installer::{DiskInfo, InstallReport, Installer, Phase, Plan, Progress, SourceInfo};

struct State {
    disks: Vec<DiskInfo>,
    source: Result<SourceInfo, String>,
    uefi: bool,
    /// What `execute` answers (default: written).
    refuse: Option<String>,
    plans: Vec<Plan>,
    calls: Vec<String>,
}

pub struct FakeInstaller {
    state: Mutex<State>,
}

impl Default for FakeInstaller {
    fn default() -> FakeInstaller {
        FakeInstaller { state: Mutex::new(State { disks: Vec::new(), source: Err("nothing to copy from".into()), uefi: false, refuse: None, plans: Vec::new(), calls: Vec::new() }) }
    }
}

impl FakeInstaller {
    pub fn new() -> FakeInstaller {
        FakeInstaller::default()
    }
    pub fn set_disks(&self, d: Vec<DiskInfo>) {
        self.state.lock().unwrap().disks = d;
    }
    pub fn set_source(&self, s: Result<SourceInfo, String>) {
        self.state.lock().unwrap().source = s;
    }
    pub fn set_uefi(&self, u: bool) {
        self.state.lock().unwrap().uefi = u;
    }
    /// Make `execute` fail with this sentence.
    pub fn refuse_execute(&self, why: &str) {
        self.state.lock().unwrap().refuse = Some(why.into());
    }
    /// Every plan `execute` was given.
    pub fn written(&self) -> Vec<Plan> {
        self.state.lock().unwrap().plans.clone()
    }
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn rec(&self, what: impl Into<String>) {
        self.state.lock().unwrap().calls.push(what.into());
    }
}

impl Installer for FakeInstaller {
    fn disks(&self) -> Vec<DiskInfo> {
        self.rec("disks");
        self.state.lock().unwrap().disks.clone()
    }
    fn source(&self, _: &[DiskInfo]) -> Result<SourceInfo, String> {
        self.rec("source");
        self.state.lock().unwrap().source.clone()
    }
    fn uefi(&self) -> bool {
        self.state.lock().unwrap().uefi
    }
    fn execute(&self, plan: &Plan, progress: &mut dyn FnMut(Progress)) -> Result<InstallReport, String> {
        self.rec(format!("execute({})", plan.target.drive.path));
        let mut s = self.state.lock().unwrap();
        if let Some(why) = &s.refuse {
            return Err(why.clone());
        }
        s.plans.push(plan.clone());
        drop(s);
        for (phase, done) in [(Phase::Writing, 0), (Phase::Writing, 1), (Phase::Verifying, 1)] {
            progress(Progress { phase, done, total: 1, elapsed: Duration::ZERO });
        }
        Ok(InstallReport { written: 1, head_sha256: String::new(), dry_run: false })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::installer::NoInstaller;

    #[test]
    fn the_fake_answers_what_it_was_given_and_a_machine_without_an_installer_refuses() {
        let f = FakeInstaller::new();
        assert!(!f.available(), "no source: nothing to install from");
        f.set_uefi(true);
        assert!(f.uefi());
        assert_eq!(f.source(&[]).unwrap_err(), "nothing to copy from");
        assert!(f.disks().is_empty());
        assert_eq!(f.calls(), vec!["disks", "source", "source", "disks"]);
        let n = NoInstaller;
        assert!(!n.available() && !n.uefi() && n.disks().is_empty());
    }
}

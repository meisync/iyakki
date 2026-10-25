//! What puts rules into the kernel. The engine only knows this trait, so tests use [`FakeBackend`] and never touch the
//! machine's real firewall.

use std::sync::{Arc, Mutex};

use crate::ruleset::Ruleset;

/// Why a change failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendError {
    /// The kernel cannot do this at all.
    Unavailable(String),
    Failed(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Unavailable(s) | BackendError::Failed(s) => f.write_str(s),
        }
    }
}

pub trait Backend: Send {
    /// Put `rs` in place of whatever table of ours exists, all or nothing.
    fn replace(&mut self, rs: &Ruleset) -> Result<(), BackendError>;
    /// Take our table out (nothing else is touched).
    fn remove(&mut self) -> Result<(), BackendError>;
    /// Packets and bytes counted as blocked.
    fn counters(&mut self) -> Result<(u64, u64), BackendError>;
}

/// The kernel's nf_tables through our netlink writer. Loads the few modules it needs first.
#[cfg(target_os = "linux")]
pub struct KernelBackend {
    modules_tried: bool,
}

/// Modules the ruleset needs (built into nf_tables.ko: meta, cmp, payload, bitwise, counter, immediate).
pub const MODULES: [&str; 4] = ["nf_tables", "nft_ct", "nft_limit", "nft_log"];

#[cfg(target_os = "linux")]
impl KernelBackend {
    pub fn new() -> KernelBackend {
        KernelBackend { modules_tried: false }
    }
    fn load_modules(&mut self) {
        if self.modules_tried {
            return;
        }
        self.modules_tried = true;
        for m in MODULES {
            let _ = std::process::Command::new("modprobe").arg(m).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        }
    }
    fn sock(&mut self) -> Result<crate::nl::NfSocket, BackendError> {
        self.load_modules();
        crate::nl::NfSocket::open(5).map_err(conv)
    }
}

#[cfg(target_os = "linux")]
impl Default for KernelBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
fn conv(e: crate::nl::NlError) -> BackendError {
    match e {
        crate::nl::NlError::Unavailable(s) => BackendError::Unavailable(s),
        crate::nl::NlError::Failed(s) => BackendError::Failed(s),
    }
}

#[cfg(target_os = "linux")]
impl Backend for KernelBackend {
    fn replace(&mut self, rs: &Ruleset) -> Result<(), BackendError> {
        self.sock()?.replace(rs).map_err(conv)
    }
    fn remove(&mut self) -> Result<(), BackendError> {
        self.sock()?.remove().map_err(conv)
    }
    fn counters(&mut self) -> Result<(u64, u64), BackendError> {
        self.sock()?.counters().map_err(conv)
    }
}

/// A stand-in for tests: remembers what was installed and can be told to fail.
#[derive(Default)]
pub struct FakeState {
    pub installed: Option<Ruleset>,
    pub calls: Vec<String>,
    pub fail_replace: Option<BackendError>,
    pub fail_remove: Option<BackendError>,
    pub packets: u64,
}

#[derive(Clone, Default)]
pub struct FakeBackend(pub Arc<Mutex<FakeState>>);

impl FakeBackend {
    pub fn installed(&self) -> Option<Ruleset> {
        self.0.lock().unwrap().installed.clone()
    }
    pub fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().calls.clone()
    }
}

impl Backend for FakeBackend {
    fn replace(&mut self, rs: &Ruleset) -> Result<(), BackendError> {
        let mut s = self.0.lock().unwrap();
        s.calls.push(format!("replace:{}", rs.rules.len()));
        if let Some(e) = s.fail_replace.clone() {
            return Err(e);
        }
        s.installed = Some(rs.clone());
        Ok(())
    }
    fn remove(&mut self) -> Result<(), BackendError> {
        let mut s = self.0.lock().unwrap();
        s.calls.push("remove".into());
        if let Some(e) = s.fail_remove.clone() {
            return Err(e);
        }
        s.installed = None;
        Ok(())
    }
    fn counters(&mut self) -> Result<(u64, u64), BackendError> {
        let s = self.0.lock().unwrap();
        Ok((s.packets, s.packets * 60))
    }
}

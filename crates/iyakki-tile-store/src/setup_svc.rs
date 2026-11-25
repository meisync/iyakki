//! The services and add-on side of the Setup tab (alpha.10 AP3, D-AP3-02 cluster, AV build and firmware groups): the managed k3s node,
//! the browser add-on's build route, the hardware firmware view and its acceptance. Plain answers only; the real port is
//! `AgentSetupSvc` in `crates/iyakki-apps/src/apps/store_port.rs`.

use std::path::{Path, PathBuf};

/// The managed development cluster's state (the supervisor's words).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum K3s {
    NotInstalled,
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed(String),
}

/// One line of the hardware firmware card's offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwOffer {
    pub ready: bool,
    pub needs_click: bool,
    pub line: String,
}

/// What the hardware firmware card shows: the summary, the vendors and their licence class, the offers, and where firmware files are
/// (a staging folder of the developer-mode link, a cable to another computer).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HwStatus {
    pub summary: String,
    pub vendors: Vec<(String, String)>,
    pub offers: Vec<HwOffer>,
    pub staging: Option<PathBuf>,
    /// A stick or folder that looks like firmware.
    pub stick: Option<PathBuf>,
    pub link_iface: Option<String>,
}

/// The add-on's supply state and what the administrator reads first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddonPlan {
    /// Something is supplied (a package, a local ffmpeg, a mirror); `false`: the default is to build here.
    pub has_source: bool,
    pub review: Vec<String>,
}

/// An accepted firmware list: its file count and the first 16 characters of its digest (the audit line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwAccepted {
    pub files: usize,
    pub digest16: String,
}

pub trait SetupSvcPort: Send + Sync {
    fn k3s_state(&self) -> K3s;
    fn k3s_start(&self, user: &str) -> Result<(), String>;
    fn k3s_stop(&self, user: &str) -> Result<(), String>;
    /// The build map of the add-on for this IYAKKI version (read from the bridge folder; empty when missing).
    fn av_map_text(&self) -> String;
    /// The consent lines of the build route for this map (an error says why the route is not available).
    fn av_consent(&self, map_text: &str) -> Result<Vec<String>, String>;
    fn av_write_request(&self, data: &Path) -> Result<(), String>;
    /// The title of the add-on build's confirmation, and the words for a machine without k3s.
    fn av_confirm_title(&self) -> String;
    fn av_no_k3s_text(&self) -> String;
    /// The add-on's supply state and its review.
    fn avaddon_plan(&self, data: &Path) -> AddonPlan;
    /// The data science toolkit's record: (state word, detail). Read only; nothing runs.
    fn datasci_state(&self, data: &Path) -> (String, String);
    /// The toolkit's self-test in the sandbox (no network); its sentence on success.
    fn datasci_verify(&self, data: &Path, user: &str) -> Result<String, String>;
    fn hw_view_now(&self, data: &Path, home: &Path) -> HwStatus;
    fn hw_accept(&self, data: &Path, with: &[String], user: &str, now: u64) -> Result<HwAccepted, String>;
}

/// Canned answers (tests): k3s not installed, no build route, no firmware.
#[derive(Debug, Default)]
pub struct FakeSetupSvc;

impl SetupSvcPort for FakeSetupSvc {
    fn k3s_state(&self) -> K3s {
        K3s::NotInstalled
    }
    fn k3s_start(&self, _user: &str) -> Result<(), String> {
        Err("fake: no k3s".into())
    }
    fn k3s_stop(&self, _user: &str) -> Result<(), String> {
        Err("fake: no k3s".into())
    }
    fn av_map_text(&self) -> String {
        String::new()
    }
    fn av_consent(&self, _map_text: &str) -> Result<Vec<String>, String> {
        Err("fake: no build route".into())
    }
    fn av_write_request(&self, _data: &Path) -> Result<(), String> {
        Ok(())
    }
    fn av_confirm_title(&self) -> String {
        String::new()
    }
    fn av_no_k3s_text(&self) -> String {
        String::new()
    }
    fn avaddon_plan(&self, _data: &Path) -> AddonPlan {
        AddonPlan::default()
    }
    fn datasci_state(&self, _data: &Path) -> (String, String) {
        (String::new(), String::new())
    }
    fn datasci_verify(&self, _data: &Path, _user: &str) -> Result<String, String> {
        Err("fake: no toolkit".into())
    }
    fn hw_view_now(&self, _data: &Path, _home: &Path) -> HwStatus {
        HwStatus::default()
    }
    fn hw_accept(&self, _data: &Path, _with: &[String], _user: &str, _now: u64) -> Result<HwAccepted, String> {
        Err("fake: no firmware".into())
    }
}

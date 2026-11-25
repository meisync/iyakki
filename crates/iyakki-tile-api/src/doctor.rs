//! The plain data and the port of the DOCTOR tile (D-S1-E4-01, option (b), as for CHECK-UP and MESH). The findings and their fix buttons live
//! here as plain mirrors of `iyakki_shared::doctor::{Finding, Fix, Action, Severity}`; the shell converts one to the other (a test keeps
//! them in step). The tile never sees the machine's facts or the support bundle: it asks the [`DoctorPort`] for findings, for the text of
//! the bundle preview and to save it. The Manual tests tab is not part of this port: the shell owns it and the tile reaches it through a
//! small pane trait of its own (`iyakki-tile-doctor`).

use std::any::Any;
use std::path::{Path, PathBuf};

/// How bad a finding is. The order is the sort order: problems first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Something that should work does not.
    Problem,
    /// Will turn into a problem, or works badly.
    Warning,
    /// Worth knowing.
    Note,
}

/// What a fix button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixAction {
    /// Open this Settings page (`tab:firewall`, the item Settings' `open_item` takes).
    Settings(&'static str),
    /// Developer mode: push its switch-off time four hours on (administrators; audited like the Settings button).
    ExtendDeveloperMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    /// The button's words.
    pub label: &'static str,
    pub action: FixAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// Stable name of the rule (tests, the bundle, the audit line of a fix).
    pub id: &'static str,
    pub severity: Severity,
    /// One plain sentence.
    pub text: String,
    /// Addresses, ports, log lines: administrators only.
    pub detail: String,
    pub fix: Option<Fix>,
}

/// One place the support bundle can be saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dest {
    pub label: String,
    pub dir: PathBuf,
}

/// What a slow read hands back: opaque to the tile, given to [`DoctorPort::finish_slow`] on the tile's own thread.
pub type SlowDone = Box<dyn Any + Send>;

/// The job of the slow reads (a thread of its own).
pub type SlowJob = Box<dyn FnOnce() -> SlowDone + Send>;

/// What the DOCTOR tile asks of the rest of the system.
pub trait DoctorPort {
    /// The signed-in person may change settings (administrators): they see the technical detail and may extend developer mode.
    fn admin(&self) -> bool;
    /// Read the cheap facts of this machine and judge them: the findings, worst first (the slow ones are not in yet).
    fn gather(&self) -> Vec<Finding>;
    /// The slow reads (a connection to each local model server, the audit log) as a job for a thread of the tile's own.
    fn slow_job(&self) -> SlowJob;
    /// The slow reads came back: fold them in and give all findings again.
    fn finish_slow(&self, done: SlowDone) -> Vec<Finding>;
    /// The exact text of the redacted support bundle for the findings last given (what the preview shows and what is saved).
    fn build_share(&self) -> String;
    /// The places the bundle can be saved: the home folder first, then the sticks DISKS opened.
    fn share_dests(&self) -> Vec<Dest>;
    /// Save the bundle built last into `dir` under its dated name; the path of the file, or why not.
    fn save_share(&self, dir: &Path) -> Result<PathBuf, String>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// Push developer mode's switch-off time `hours` on; the time it now switches off, or why not.
    fn extend_devmode(&self, hours: u32) -> Result<i64, String>;
    /// "Developer mode now switches off at ..." for that time.
    fn stamp(&self, unix: i64) -> String;
}

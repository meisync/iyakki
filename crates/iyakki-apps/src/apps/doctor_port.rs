//! What the DOCTOR tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`doctor.rs`) names no `Services`, no session and no audit log: it asks a [`DoctorPort`] for the machine's facts, the slow
//! probe's input, the inputs of the support bundle, the person's home folder, the audit line and the developer-mode extension. The real
//! port ([`CxPort`]) is the only place that reads the session and calls the core (`doctor_facts.rs` does the reading). [`Parts`] stays the
//! way tests replace each reader.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_shared::diagshare::Inputs;
use iyakki_shared::doctor::{Facts, Finding};
use iyakki_shared::manualtest::{EvidenceStore, Who};

use super::doctor_facts::{self as facts, SlowIn, SlowProbe};
use super::AppCx;

pub trait DoctorPort {
    /// The signed-in person may change settings (administrators): they see the technical detail and may extend developer mode.
    fn admin(&self) -> bool;
    /// The cheap facts of this machine (the slow ones are left empty).
    fn gather(&self) -> Facts;
    /// What the slow thread needs to read.
    fn slow_input(&self) -> SlowIn;
    /// What the support bundle collects (before redaction).
    fn bundle_inputs(&self, found: &[Finding]) -> Inputs;
    /// The person's home folder (the first place a bundle can be saved).
    fn home(&self) -> PathBuf;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// Push developer mode's switch-off time `hours` on; the time it now switches off, or why not.
    fn extend_devmode(&self, hours: u32) -> Result<i64, String>;

    // ---- the "Manual tests" tab (doctor_manual.rs): checklists that ship read-only with the image, answers kept in the person's home ----
    /// The bundled checklists as (file name, text); none on a computer that did not boot from an image.
    fn manual_files(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// The signed-in person, this machine's name and the image's commit.
    fn manual_who(&self) -> Who {
        Who::default()
    }
    /// The hardware facts of this machine (the model and parts, one text) for the "this machine" filter.
    fn manual_hardware(&self) -> String {
        String::new()
    }
    /// The person's files by path inside the home folder, through the core's file handles (scope and audit are the core's).
    fn store_read(&self, _rel: &str) -> Result<Option<String>, String> {
        Err("this core has no file browser".into())
    }
    /// Save a text file; the core asks the person, and the tab only calls this after the person's own click or key.
    fn store_write(&self, _rel: &str, _text: &str) -> Result<(), String> {
        Err("this core has no file browser".into())
    }
    fn store_list(&self, _dir: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    fn store_folders(&self, _dir: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}

/// The port as an [`EvidenceStore`].
pub struct PortStore<'a>(pub &'a dyn DoctorPort);

impl EvidenceStore for PortStore<'_> {
    fn read(&self, rel: &str) -> Result<Option<String>, String> {
        self.0.store_read(rel)
    }
    fn write(&self, rel: &str, text: &str) -> Result<(), String> {
        self.0.store_write(rel, text)
    }
    fn list(&self, dir: &str) -> Result<Vec<String>, String> {
        self.0.store_list(dir)
    }
    fn folders(&self, dir: &str) -> Result<Vec<String>, String> {
        self.0.store_folders(dir)
    }
}

/// How the tile reads the world; tests replace every part. The closures get the port (the real ones ask it, the test ones ignore it).
#[derive(Clone)]
pub struct Parts {
    pub gather: Arc<dyn Fn(&dyn DoctorPort) -> Facts + Send + Sync>,
    pub slow: SlowProbe,
    pub inputs: Arc<dyn Fn(&dyn DoctorPort, &[Finding]) -> Inputs + Send + Sync>,
    /// The person's home folder (the first place a bundle can be saved).
    pub home: Arc<dyn Fn(&dyn DoctorPort) -> PathBuf + Send + Sync>,
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Parts {
    pub fn real() -> Parts {
        Parts {
            gather: Arc::new(|p| p.gather()),
            slow: Arc::new(facts::slow_probe),
            inputs: Arc::new(|p, found| p.bundle_inputs(found)),
            home: Arc::new(|p| p.home()),
            now: Arc::new(iyakki_clock::now_unix),
        }
    }
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl DoctorPort for CxPort<'_> {
    fn admin(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn gather(&self) -> Facts {
        facts::gather(self.0)
    }
    fn slow_input(&self) -> SlowIn {
        facts::slow_input(self.0)
    }
    fn bundle_inputs(&self, found: &[Finding]) -> Inputs {
        facts::bundle_inputs(self.0, found)
    }
    fn home(&self) -> PathBuf {
        self.0.core.file_scope().home_root(&self.0.session.user, self.0.data_dir())
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    fn extend_devmode(&self, hours: u32) -> Result<i64, String> {
        self.0.agent.devmode().extend(self.0.data_dir(), &self.0.session.policy, &self.0.session.user, hours)
    }
    fn manual_files(&self) -> Vec<(String, String)> {
        facts::manual_files(std::path::Path::new(iyakki_shared::manualtest::BUNDLED_DIR))
    }
    fn manual_who(&self) -> Who {
        facts::manual_who(self.0)
    }
    fn manual_hardware(&self) -> String {
        iyakki_shared::doctor::hardware_text()
    }
    fn store_read(&self, rel: &str) -> Result<Option<String>, String> {
        match self.0.core.files().read_text(&self.0.session.user, rel, iyakki_core_api::files::MAX_READ) {
            Ok(t) => Ok(Some(t)),
            // a file that is not there is not an error; anything else (a link, too big, no files) is
            Err(e) if e.contains("not found") || e.contains("no such") || e.contains("not there") || e.contains("cannot be read") => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn store_write(&self, rel: &str, text: &str) -> Result<(), String> {
        use iyakki_core_api::files::{self as f, FileChange};
        let files = self.0.core.files();
        let user = &self.0.session.user;
        // the folders first (the core's save does not make them); each is a change the person's own click approves
        let names = f::check_rel(rel)?;
        let mut dir = String::new();
        for n in &names[..names.len().saturating_sub(1)] {
            let next = f::join(&dir, n);
            if files.list(user, &next).is_err() {
                let ask = files.change(user, &FileChange::MakeDir { dir: dir.clone(), name: (*n).to_string() })?;
                files.decide(user, &ask.call, true)?;
            }
            dir = next;
        }
        let ask = files.change(user, &FileChange::Save { path: rel.to_string(), text: text.to_string() })?;
        files.decide(user, &ask.call, true)?;
        self.audit("manual.evidence.write", rel);
        Ok(())
    }
    fn store_list(&self, dir: &str) -> Result<Vec<String>, String> {
        match self.0.core.files().list(&self.0.session.user, dir) {
            Ok(l) => Ok(l.entries.into_iter().filter(|e| !e.is_dir).map(|e| e.name).collect()),
            Err(_) => Ok(Vec::new()),
        }
    }
    fn store_folders(&self, dir: &str) -> Result<Vec<String>, String> {
        match self.0.core.files().list(&self.0.session.user, dir) {
            Ok(l) => Ok(l.entries.into_iter().filter(|e| e.is_dir).map(|e| e.name).collect()),
            Err(_) => Ok(Vec::new()),
        }
    }
}

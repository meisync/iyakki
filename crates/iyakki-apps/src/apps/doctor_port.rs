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
    /// The runner's checklists (`dist/autotest/checklists/<ROW>.json`) as (file name, text); none when the image does not carry them.
    fn manual_checklists(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// The bytes of an example screenshot a step names, relative to the folder of that source; `None` when it is not on this computer.
    fn manual_shot(&self, _source: iyakki_shared::manualtest::Source, _rel: &str) -> Option<Vec<u8>> {
        None
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
    fn manual_checklists(&self) -> Vec<(String, String)> {
        facts::manual_files_ext(std::path::Path::new(iyakki_shared::manualtest::BUNDLED_CHECKLIST_DIR), ".json")
    }
    fn manual_shot(&self, source: iyakki_shared::manualtest::Source, rel: &str) -> Option<Vec<u8>> {
        use iyakki_shared::manualtest as mt;
        let dir = if source == mt::Source::Runner { mt::BUNDLED_CHECKLIST_DIR } else { mt::BUNDLED_DIR };
        facts::manual_shot(std::path::Path::new(dir), rel)
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

// ---- the tile crate's port (D-S1-E4-01, option (b)): plain mirrors in iyakki-tile-api, the facts and the bundle stay on this side ----------

use std::any::Any;
use std::cell::RefCell;

use iyakki_shared::diagshare::{self, Bundle};
use iyakki_shared::doctor::{self, Action, Severity};
use iyakki_tile_api::doctor as api;

pub fn to_severity(s: Severity) -> api::Severity {
    match s {
        Severity::Problem => api::Severity::Problem,
        Severity::Warning => api::Severity::Warning,
        Severity::Note => api::Severity::Note,
    }
}

pub fn to_finding(f: &Finding) -> api::Finding {
    api::Finding {
        id: f.id,
        severity: to_severity(f.severity),
        text: f.text.clone(),
        detail: f.detail.clone(),
        fix: f.fix.as_ref().map(|x| api::Fix {
            label: x.label,
            action: match x.action {
                Action::Settings(i) => api::FixAction::Settings(i),
                Action::ExtendDeveloperMode => api::FixAction::ExtendDeveloperMode,
            },
        }),
    }
}

/// What the shell keeps for the tile between calls: the facts of the last reading, the findings judged from them, the bundle built last.
#[derive(Default)]
pub struct State {
    pub facts: RefCell<Facts>,
    pub found: RefCell<Vec<Finding>>,
    pub bundle: RefCell<Option<Bundle>>,
}

/// The tile's port over the shell's [`DoctorPort`] and the readers in [`Parts`].
pub struct Bridge<'a> {
    pub port: &'a dyn DoctorPort,
    pub parts: &'a Parts,
    pub st: &'a State,
}

impl Bridge<'_> {
    fn rediagnose(&self) -> Vec<api::Finding> {
        let found = doctor::diagnose(&self.st.facts.borrow());
        let out = found.iter().map(to_finding).collect();
        *self.st.found.borrow_mut() = found;
        out
    }
}

impl api::DoctorPort for Bridge<'_> {
    fn admin(&self) -> bool {
        self.port.admin()
    }
    fn gather(&self) -> Vec<api::Finding> {
        *self.st.facts.borrow_mut() = (self.parts.gather)(self.port);
        self.rediagnose()
    }
    fn slow_job(&self) -> api::SlowJob {
        let inp = self.port.slow_input();
        let probe = self.parts.slow.clone();
        Box::new(move || Box::new(probe(inp)) as Box<dyn Any + Send>)
    }
    fn finish_slow(&self, done: api::SlowDone) -> Vec<api::Finding> {
        if let Ok(out) = done.downcast::<facts::SlowOut>() {
            out.apply(&mut self.st.facts.borrow_mut());
        }
        self.rediagnose()
    }
    fn build_share(&self) -> String {
        let inp = (self.parts.inputs)(self.port, &self.st.found.borrow());
        let bundle = diagshare::build(&inp);
        let text = diagshare::preview(&bundle);
        *self.st.bundle.borrow_mut() = Some(bundle);
        text
    }
    fn share_dests(&self) -> Vec<api::Dest> {
        let home = (self.parts.home)(self.port);
        let mut dests = vec![api::Dest { label: "my home folder".into(), dir: home.clone() }];
        // USB sticks and cards that DISKS opened in the home folder
        let mut media: Vec<api::Dest> = std::fs::read_dir(home.join("media"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| api::Dest { label: e.file_name().to_string_lossy().into_owned(), dir: e.path() })
            .collect();
        media.sort_by(|a, b| a.label.cmp(&b.label));
        media.truncate(4);
        dests.extend(media);
        dests
    }
    fn save_share(&self, dir: &std::path::Path) -> Result<PathBuf, String> {
        let now = (self.parts.now)();
        let b = self.st.bundle.borrow();
        let b = b.as_ref().ok_or_else(|| "there is nothing to save".to_string())?;
        diagshare::save(b, dir, &diagshare::file_name(now))
    }
    fn audit(&self, action: &str, subject: &str) {
        self.port.audit(action, subject);
    }
    fn extend_devmode(&self, hours: u32) -> Result<i64, String> {
        self.port.extend_devmode(hours)
    }
    fn stamp(&self, unix: i64) -> String {
        iyakki_shared::devmode::stamp(unix)
    }
}

/// The port between frames, where the shell hands over no context (`wants_redraw`): only the state is read, nothing is asked of the world.
pub struct Held {
    pub admin: bool,
}

impl DoctorPort for Held {
    fn admin(&self) -> bool {
        self.admin
    }
    fn gather(&self) -> Facts {
        Facts::default()
    }
    fn slow_input(&self) -> SlowIn {
        SlowIn { servers: Vec::new(), audit_recent: Arc::new(|_| Vec::new()), dev_port: None }
    }
    fn bundle_inputs(&self, _found: &[Finding]) -> Inputs {
        Inputs::default()
    }
    fn home(&self) -> PathBuf {
        PathBuf::new()
    }
    fn audit(&self, _action: &str, _subject: &str) {}
    fn extend_devmode(&self, _hours: u32) -> Result<i64, String> {
        Err("needs the shell's context".into())
    }
}

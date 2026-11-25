//! What the LAB tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile (`app.rs`, `ui.rs`, `pic.rs`) names no `Services`, no session and no audit log: it asks a [`LabPort`] for the rig it
//! works with (the picture and vision backends the core serves, the frame source, the camera and the screen), the model catalogue, the
//! vision check, the files it may read and write, the Lemonade lists and the audit line. The real port (`CxPort` in the shell crate, `lab_port.rs`) is the only place that
//! reads the session and calls the services.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_lab_model::{Catalogue, Sees};

use crate::app::Rig;

pub trait LabPort {
    /// The backends and sources the tile works with (a fake rig in tests).
    fn rig(&self) -> Rig;
    /// The models of every tab as the services know them (providers, Lemonade lists); no network.
    fn catalogue(&self) -> Catalogue;
    /// May this model take pictures in (asks the provider's list, nothing is sent).
    fn check_vision(&self, cat: &Catalogue, provider: &str, model: &str) -> Result<Sees, String>;
    /// Resolve a typed path inside the folders the person may read (`home` replaces the account's own home in tests) and check the read.
    fn read_path(&self, home: Option<PathBuf>, input: &str) -> Result<PathBuf, String>;
    /// Write `bytes` as `dir/name` after checking the person may write there; returns where it went.
    fn save_file(&self, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String>;
    /// The enabled Lemonade providers whose model lists can be read again.
    fn lemonade_ids(&self) -> Vec<String>;
    /// Read one Lemonade provider's model list again (blocks: call it from a worker thread).
    fn refresh_lemonade(&self) -> Arc<dyn Fn(&str) + Send + Sync>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

//! The PDF facade: what the PDF tile asks of the shell. The tile reads the file itself (the bytes go to its worker thread); the shell decides
//! whether the person may read it, tells where their Downloads folder is and records the audit line. A fake in the tile's tests.

use std::path::{Path, PathBuf};

pub trait PdfFiles: Send + Sync {
    /// The person's Downloads folder (listed on the empty screen), when they have one.
    fn downloads_dir(&self) -> Option<PathBuf>;
    /// May this person read this file under the file rules of their policy? `Err` is the sentence to show.
    fn check_read(&self, path: &Path) -> Result<(), String>;
    /// The person opened `file_name`: the audit log line `pdf.open`.
    fn opened(&self, file_name: &str);
    /// May this person write this file (a filled-in copy) under the file rules of their policy? `Err` is the sentence to show.
    /// The default refuses: a host that does not say so cannot save.
    fn check_write(&self, _path: &Path) -> Result<(), String> {
        Err("saving is not available here".into())
    }
    /// A filled-in copy named `file_name` was saved: the audit log line `pdf.form.save`.
    fn saved(&self, _file_name: &str) {}
}

/// No Downloads folder, every file allowed, nothing recorded.
pub struct NoPdfFiles;

impl PdfFiles for NoPdfFiles {
    fn downloads_dir(&self) -> Option<PathBuf> {
        None
    }
    fn check_read(&self, _: &Path) -> Result<(), String> {
        Ok(())
    }
    fn opened(&self, _: &str) {}
}

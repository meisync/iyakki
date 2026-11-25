//! What the DEV TOOLS tile asks of the rest of the system: the person's files under their file rules, the clipboard, the picture writer and the
//! clock for a saved QR picture, the audit line, and the two network tools (ping and the port checker) that stay in the shell (D-S1-6: they open
//! sockets). The tile itself reads no file, opens no socket and names no session.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A running network check (ping or the port checker): the lines so far, whether it ended, and a stop.
pub trait NetJob: Send + Sync {
    fn finished(&self) -> bool;
    fn stop(&self);
    /// The lines so far, in order.
    fn text(&self) -> String;
}

pub trait DevToolsPort {
    /// The file `path` as UTF-8 text when the account may read it (at most `max` bytes); the error is a sentence for the person.
    fn read_text_file(&self, path: &str, max: u64) -> Result<String, String>;
    /// The person's home folder (the QR pictures go to its `Pictures`).
    fn home_dir(&self) -> PathBuf;
    /// Writes a picture under the file rules (never over a file: a number is added to the name), and returns where it went.
    fn save_picture(&self, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String>;
    /// A PNG of RGBA pixels with the pixel density `dpi` in its header.
    fn png_with_dpi(&self, w: u32, h: u32, rgba: &[u8], dpi: f32) -> Result<Vec<u8>, String>;
    /// `20261105-143012`: the local time for a file name.
    fn stamp(&self) -> String;
    /// One audit line as the signed-in person.
    fn audit(&self, action: &str, detail: &str);
    /// Ping `host` `count` times on a background thread (the shell's ICMP or TCP timing).
    fn start_ping(&self, host: &str, count: usize) -> Result<Arc<dyn NetJob>, String>;
    /// Check the ports of `spec` (`22,80,8000-8010`) on `host` on a background thread.
    fn start_ports(&self, host: &str, spec: &str) -> Result<Arc<dyn NetJob>, String>;
    /// The text on the clipboard, if any.
    fn clipboard_paste(&self) -> Option<String> {
        iyakki_uikit::ui::clipboard::paste()
    }
    /// Puts text on the clipboard.
    fn clipboard_copy(&self, text: &str) {
        iyakki_uikit::ui::clipboard::copy(text)
    }
}

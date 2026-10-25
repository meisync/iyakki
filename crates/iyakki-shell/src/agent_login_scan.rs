//! Reading the sign-in code from a QR code held up to the camera: the state machine behind the dialog's "Scan with camera" button.
//!
//! The person shows the code on a phone screen; the camera is opened only after they say yes, frames are looked at in memory
//! with [`iyakki_agent::qr_decode`] and never stored or sent, and the camera is closed as soon as a code is read, on Esc, when the
//! dialog goes away and after [`SCAN_SECS`] seconds. What was read goes into the code field only when it has the shape of a
//! sign-in code ([`validate`]); an address is never put there and never opened, and the text is never logged, saved or sent.

use std::sync::Arc;
use std::time::Duration;

use iyakki_agent::extagent::login;

pub use crate::scan_core::{decode_frame, CamSource, Step, SCAN_SECS};
use crate::scan_core::Session;

/// The sign-in dialog's scan: the shared camera session plus the check that what was read is a sign-in code.
pub struct Scan {
    core: Session,
}

impl std::ops::Deref for Scan {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.core
    }
}

impl std::ops::DerefMut for Scan {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.core
    }
}

impl Scan {
    pub fn new(src: Arc<CamSource>) -> Scan {
        Scan { core: Session::new(src, "There is no camera on this device. Copy the code from the page and paste it instead.") }
    }

    /// A shorter time limit (tests).
    pub fn with_limit(mut self, limit: Duration) -> Scan {
        self.core = self.core.with_limit(limit);
        self
    }

    /// A fixed decoder budget per frame (tests).
    pub fn with_budget(mut self, budget: Duration) -> Scan {
        self.core = self.core.with_budget(budget);
        self
    }

    /// Look at the camera: the code to fill in when a valid one was read, else `None`. Called on every heartbeat.
    pub fn poll(&mut self, want_state: bool) -> Option<String> {
        self.core.poll(|bytes| {
            let code = validate(bytes, want_state)?;
            let msg = format!("Code read from the QR code ({} characters). The camera is off. Check it, then press Enter.", code.chars().count());
            Ok((code, msg))
        })
    }
}

/// What may go into the code field: the shape of a sign-in code, or a plain reason why not. The reasons never repeat the scanned
/// text, and an address is refused outright (it is never filled in and never opened).
pub fn validate(bytes: &[u8], want_state: bool) -> Result<String, String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Err("That QR code does not hold text, so it is not a sign-in code.".to_string());
    };
    let lower = text.trim().to_ascii_lowercase();
    if lower.contains("://") || lower.starts_with("www.") || lower.starts_with("mailto:") || lower.starts_with("javascript:") || lower.starts_with("data:") {
        return Err("That QR code holds a web address, not a sign-in code. Nothing was opened or filled in. Show the QR code of the code itself.".to_string());
    }
    let c = login::check_code(text, want_state);
    if c.problem.is_some() || c.from_url || c.len < 20 || c.len > 2000 || (want_state && !c.code.contains('#')) {
        return Err("That QR code does not look like a sign-in code. Nothing was filled in.".to_string());
    }
    Ok(c.code)
}

#[cfg(test)]
#[path = "agent_login_scan_tests.rs"]
pub(crate) mod tests;

//! The lock in the browser's address bar opens the certificate the site presents (F-BROW-98): click the "Secure" badge, or press Ctrl+U.
//! The same handshake-only probe as Settings -> Browser -> "Check a site's certificate" and the assistant's `browser_certificate_view`
//! runs on a thread of its own (the page never waits for it), the account's list of allowed sites is applied first, and the look is
//! audited without the answer. A page that is not secure has no certificate to show: the panel says so. The certificate is the one the
//! tab's own HTTP/2 connection was given when the browser has one (`iyakki_proxy::certseen`), else a fresh handshake.
use std::sync::{Arc, Condvar, Mutex};

pub const TAB_CERT: usize = 520;

type Answer = Result<Vec<String>, String>;

/// The open panel: the site it is about and the answer, which a thread fills in.
pub struct CertPanel {
    pub host: String,
    slot: Arc<(Mutex<Option<Answer>>, Condvar)>,
}

impl CertPanel {
    /// Start asking `host` (port as given) on a thread of its own.
    pub fn ask(host: &str, port: u16) -> CertPanel {
        let slot: Arc<(Mutex<Option<Answer>>, Condvar)> = Arc::new((Mutex::new(None), Condvar::new()));
        let (s2, h) = (slot.clone(), host.to_string());
        std::thread::spawn(move || {
            // the certificate the tab's own connection was given when the browser has one, else a fresh handshake
            let r = match crate::apps::chat_settings::cert_seen(&h, port) {
                Some(seen) => seen.map(|v| std::iter::once("The certificate this tab's connection was given.".to_string()).chain(v.text().lines().map(str::to_string)).collect()),
                None => crate::apps::chat_settings::cert_probe(&h, port).map(|v| v.text().lines().map(str::to_string).collect()),
            };
            *s2.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            s2.1.notify_all();
        });
        CertPanel { host: host.to_string(), slot }
    }

    /// A panel that says why there is nothing to ask.
    pub fn said(host: &str, why: &str) -> CertPanel {
        CertPanel { host: host.to_string(), slot: Arc::new((Mutex::new(Some(Err(why.to_string()))), Condvar::new())) }
    }

    /// The lines to draw now: the title, then the certificate, the reason it is missing, or a waiting line.
    pub fn lines(&self) -> Vec<String> {
        let g = self.slot.0.lock().unwrap_or_else(|e| e.into_inner());
        match &*g {
            None => vec![format!("Asking {} for its certificate...", self.host)],
            Some(Ok(l)) => l.clone(),
            Some(Err(e)) => vec![e.clone()],
        }
    }

    /// Whether the answer is in.
    pub fn done(&self) -> bool {
        self.slot.0.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// Block until the answer is in (tests; the app never waits).
    #[cfg(test)]
    pub fn wait(&self) {
        let mut g = self.slot.0.lock().unwrap_or_else(|e| e.into_inner());
        while g.is_none() {
            g = self.slot.1.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

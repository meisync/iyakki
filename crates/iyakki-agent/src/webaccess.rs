//! "Serve this desktop to browsers" (Settings -> Remote access): the switch, the port and what the page shows. The listener itself
//! is the browser front end (`iyakki-remote`); the thread that opens and closes it with the switch, and the firewall rule, are in
//! `iyakki_remote::webswitch`.
//!
//! * Off by default. Always HTTPS (the certificate is the one in `<data>/tls`, self-signed the first time); plain HTTP stays a
//!   boot-time choice (`iyakki.web=http:...`, for a proxy that does the TLS) and is never offered by the switch.
//! * Signing in uses this machine's accounts. The switch is refused while no administrator account with a password exists.
//! * When the front end was already started at boot (`iyakki.web`, `--web`, `IYAKKI_WEB`) the switch does not start a second one:
//!   it says "on (set at boot)" and leaves that one alone.

use std::path::Path;
use std::sync::atomic::{AtomicU16, Ordering};

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 8443;
pub const STATUS_FILE: &str = "webaccess.json";
/// Startup registry id of the listener (Settings -> Startup).
pub const SERVICE_ID: &str = "webaccess";

/// What the audit line says and the page repeats before the person switches it on.
pub const EXPOSES: &str = "Anyone on this network who knows a sign-in can use this desktop. Sign-in uses this machine's accounts, so every account with a password can open its own session from another computer. Traffic is encrypted (HTTPS); the first time, compare the certificate fingerprint below.";

static BOOT_PORT: AtomicU16 = AtomicU16::new(0);

/// The front end was started by the boot option, the command line or the environment, on this port. Called by the starter.
pub fn note_boot(port: u16) {
    BOOT_PORT.store(port, Ordering::SeqCst);
}

/// The port of the front end started at boot, if there is one.
pub fn boot_port() -> Option<u16> {
    Some(BOOT_PORT.load(Ordering::SeqCst)).filter(|p| *p != 0)
}

/// The port to use for a stored value (0: the default).
pub fn effective_port(stored: u16) -> u16 {
    if stored == 0 { DEFAULT_PORT } else { stored }
}

/// A port a person may choose: 1024 to 65535 (ports below belong to the system's own services).
pub fn check_port(p: u16) -> Result<u16, String> {
    if p == 0 {
        return Ok(0);
    }
    if p < 1024 {
        return Err("Choose a port from 1024 to 65535 (lower ports belong to the system's own services).".into());
    }
    Ok(p)
}

/// Parse the text of the port field: empty means the default.
pub fn parse_port(t: &str) -> Result<u16, String> {
    let t = t.trim();
    if t.is_empty() {
        return Ok(0);
    }
    let p: u16 = t.parse().map_err(|_| "The port is a number from 1024 to 65535.".to_string())?;
    check_port(p)
}

/// Is there an administrator who can sign in with a password? (`users`: `(is administrator, has a password hash)`.)
pub fn has_admin_password(users: impl IntoIterator<Item = (bool, bool)>) -> bool {
    users.into_iter().any(|(admin, hash)| admin && hash)
}

/// What the listener reports about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Listening now.
    pub listening: bool,
    pub port: u16,
    /// The listener was started at boot, not by the switch.
    pub boot: bool,
    /// Why it is not listening although it is wanted (port in use, certificate), else empty.
    pub error: String,
}

impl Status {
    /// One line for the screens, given whether the switch is on.
    pub fn line(&self, switch_on: bool) -> String {
        if self.listening && self.boot {
            format!("on (set at boot), port {}", self.port)
        } else if self.listening {
            format!("on, port {}", self.port)
        } else if !self.error.is_empty() {
            format!("not listening: {}", self.error)
        } else if switch_on {
            "starting".into()
        } else {
            "off: nothing listens".into()
        }
    }
}

pub fn write_status(dir: &Path, s: &Status) {
    let tmp = dir.join(format!("{STATUS_FILE}.tmp"));
    if let Ok(t) = serde_json::to_string(s) {
        if std::fs::write(&tmp, t).and_then(|_| std::fs::rename(&tmp, dir.join(STATUS_FILE))).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// The status as the watcher last wrote it; a front end started at boot counts as listening even if the watcher is not running.
pub fn read_status(dir: &Path) -> Status {
    let mut s: Status = std::fs::read_to_string(dir.join(STATUS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if let Some(p) = boot_port() {
        s = Status { listening: true, port: p, boot: true, error: String::new() };
    }
    s
}

/// The addresses to type in a browser: the stable name first (`<label>.local`), then each IPv4 address (loopback left out).
pub fn urls(label: &str, ips: &[String], port: u16) -> Vec<String> {
    let mut v = vec![format!("https://{label}.local:{port}")];
    for ip in ips.iter().filter(|i| !i.starts_with("127.")) {
        let u = format!("https://{ip}:{port}");
        if !v.contains(&u) {
            v.push(u);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_default_and_limits() {
        assert_eq!(effective_port(0), 8443);
        assert_eq!(effective_port(9000), 9000);
        assert_eq!(parse_port(""), Ok(0));
        assert_eq!(parse_port(" 9443 "), Ok(9443));
        assert!(parse_port("80").is_err() && parse_port("abc").is_err() && parse_port("70000").is_err());
    }

    #[test]
    fn needs_an_administrator_with_a_password() {
        assert!(!has_admin_password([]));
        assert!(!has_admin_password([(false, true), (true, false)]));
        assert!(has_admin_password([(false, false), (true, true)]));
    }

    #[test]
    fn the_status_line_says_what_is_happening() {
        let mut s = Status::default();
        assert_eq!(s.line(false), "off: nothing listens");
        assert_eq!(s.line(true), "starting");
        s.error = "port 8443 is in use".into();
        assert!(s.line(true).contains("port 8443 is in use"));
        s = Status { listening: true, port: 8443, boot: true, error: String::new() };
        assert_eq!(s.line(true), "on (set at boot), port 8443");
        s.boot = false;
        assert_eq!(s.line(true), "on, port 8443");
    }

    #[test]
    fn urls_list_the_name_then_the_addresses_without_loopback() {
        let u = urls("iyakki-0a1b2c", &["127.0.0.1".into(), "192.0.2.10".into(), "192.0.2.10".into()], 8443);
        assert_eq!(u, vec!["https://iyakki-0a1b2c.local:8443", "https://192.0.2.10:8443"]);
    }

    #[test]
    fn status_round_trips_through_the_file() {
        let d = std::env::temp_dir().join(format!("iyakki-webaccess-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let s = Status { listening: false, port: 9443, boot: false, error: "in use".into() };
        write_status(&d, &s);
        assert_eq!(read_status(&d), s);
        let _ = std::fs::remove_dir_all(&d);
    }
}

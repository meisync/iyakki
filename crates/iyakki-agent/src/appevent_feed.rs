//! The real readers that feed the further event sources (F-SDK-204 to 207): what the audit log says about sign-ins and pairing, the
//! certificates the machine keeps, the update bundles waiting on the disk, and the wake phrase counter. Every function takes the text or
//! the folder it reads, so a test gives it a made-up one; a missing file says nothing.

use std::collections::BTreeMap;
use std::path::Path;

/// Running totals read from the audit log (a person's name never leaves this function: only counts do).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditTotals {
    /// `login.failed` lines.
    pub failed_signins: u64,
    /// `devicekey.enroll` lines: a new paired device.
    pub new_paired_devices: u64,
}

/// Count the audit log's JSON lines. Failed sign-ins carry no person (the name tried may not exist) and are machine-wide, as are pairings.
pub fn audit_totals(text: &str) -> AuditTotals {
    let mut t = AuditTotals::default();
    for l in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        match v.get("event").and_then(|e| e.as_str()) {
            Some("login.failed") => t.failed_signins += 1,
            Some("devicekey.enroll") => t.new_paired_devices += 1,
            _ => {}
        }
    }
    t
}

/// Days left of each certificate the machine keeps under `<dir>` (`tls/cert.pem`, `cert.pem` and `tls/*.crt`), keyed by the file's short name.
pub fn cert_days(dir: &Path, now_unix: i64) -> BTreeMap<String, i64> {
    let mut files = vec![dir.join("tls/cert.pem"), dir.join("cert.pem")];
    for e in std::fs::read_dir(dir.join("tls")).into_iter().flatten().flatten() {
        if e.path().extension().is_some_and(|x| x == "crt") {
            files.push(e.path());
        }
    }
    let mut out = BTreeMap::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let Some(not_after) = first_cert_not_after(&text) else { continue };
        let name = f.strip_prefix(dir).unwrap_or(&f).to_string_lossy().to_string();
        out.insert(name, crate::appevent_more::days_left(not_after, now_unix));
    }
    out
}

/// The `notAfter` of the first certificate block of a PEM text.
pub fn first_cert_not_after(pem: &str) -> Option<i64> {
    let body: String = pem.split("-----BEGIN CERTIFICATE-----").nth(1)?.split("-----END CERTIFICATE-----").next()?.lines().map(str::trim).collect();
    let der = iyakki_shared::b64::b64_decode(&body)?;
    iyakki_link::certinfo::read_cert(&der).map(|c| c.not_after)
}

/// The update phase of the system: `downloaded` while a checked bundle waits unpacked under the update centre's folder, else nothing.
pub fn update_phases(data: &Path) -> BTreeMap<String, String> {
    let waiting = std::fs::read_dir(crate::sysupdate::staged_root(data)).into_iter().flatten().flatten().any(|e| e.path().is_dir());
    if waiting {
        BTreeMap::from([("system".to_string(), "downloaded".to_string())])
    } else {
        BTreeMap::new()
    }
}

#[cfg(test)]
#[path = "appevent_feed_tests.rs"]
mod tests;

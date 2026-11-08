//! The record of what this install runs, per system component (`components-installed.json` in the data folder; docs/development/
//! COMPONENT-UPDATES-PLAN.md).
//!
//! Two writers, and only these:
//!
//! * [`record_applied`] after an apply that was **verified** (compapply.rs): the version and SHA-256 of what was written. A component that
//!   needs a restart (kernel, boot, base, and the core, which is stored for the next start) is marked *awaiting* with the id of the
//!   start it was written in, so the record is not "corrected" back before the person has restarted.
//! * [`correct`] from facts, at boot and whenever the record is read for display: the running kernel's release, the `VERSION` of
//!   `/etc/iyakki-release` and the version the running program was built as. A record that disagrees with a fact is replaced by the fact
//!   (its SHA-256 and file are dropped, so the next plan fetches a whole file rather than trust a difference against an unknown file); a
//!   component with no record takes the fact. So a stale record cannot lie: after a rollback, a fall-back start or a reinstall, the record
//!   follows what really runs.
//!
//! Which fact stands for which component (a store's versions for these parts follow the same convention): `kernel` is the kernel release
//! (its numeric start, `6.12.112` of `6.12.112-iyakki`); `core` and `kernel-layer` are the version the running program was built as
//! (the workspace version, `relinfo::VERSION`; a store may use `0.1.0-alpha.3.1107` for a build of it); `base` and `boot` are the `VERSION` of
//! the release file. Every fact is passed in ([`Facts`]), so tests need no system files.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::components::{Installed, INSTALLED_FILE};

/// What the running system says about itself. `None` means "could not be read": the record is then left alone for that part.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// The kernel release as `uname -r` gives it.
    pub kernel_release: Option<String>,
    /// `VERSION=` of the release file (`/etc/iyakki-release`).
    pub release_version: Option<String>,
    /// The version the running program was built as.
    pub core_version: Option<String>,
    /// The id of this start (the kernel's boot id).
    pub boot_id: Option<String>,
    /// The file the running program is installed as (its SHA-256 is read at boot when the record has none).
    pub core_file: Option<PathBuf>,
}

impl Facts {
    /// The running system's facts.
    pub fn real() -> Facts {
        let release_version = std::fs::read_to_string(crate::relinfo::RELEASE_FILE).ok().map(|t| crate::relinfo::ReleaseInfo::parse(&t).version).filter(|v| !v.is_empty());
        let core_file = Some(PathBuf::from("/usr/bin/iyakki")).filter(|p| p.is_file()).or_else(|| std::env::current_exe().ok());
        let boot_id = Some(crate::devmode::centre::boot_id_now()).filter(|b| !b.is_empty());
        Facts { kernel_release: uname_release(), release_version, core_version: Some(crate::relinfo::VERSION.to_string()), boot_id, core_file }
    }
}

fn uname_release() -> Option<String> {
    // SAFETY: `utsname` is plain bytes; uname fills it and NUL-terminates each field.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 {
        return None;
    }
    let bytes: Vec<u8> = u.release.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8(bytes).ok().filter(|s| !s.is_empty())
}

/// `6.12.112` of `6.12.112-iyakki`: the digits-and-dots start of a release string.
pub fn numeric_start(s: &str) -> Option<String> {
    let t: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let t = t.trim_end_matches('.').to_string();
    (!t.is_empty()).then_some(t)
}

/// Does a recorded version describe the running fact? Equal, or one is the other followed by `.`, `-` or `+` and more (a build of that
/// version).
pub fn consistent(recorded: &str, fact: &str) -> bool {
    let starts = |long: &str, short: &str| long == short || long.strip_prefix(short).is_some_and(|rest| rest.starts_with(['.', '-', '+']));
    starts(recorded, fact) || starts(fact, recorded)
}

fn sha_of_file(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 64 << 10]);
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            return Some(crate::pkg::hex(&h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

/// Make `inst` agree with the facts. Returns one plain sentence per change. `hash_core` also reads the running program's SHA-256 when the
/// record has none (80 MB: done at boot, not on every look).
pub fn correct(inst: &mut Installed, f: &Facts, hash_core: bool) -> Vec<String> {
    let mut notes = vec![];
    let kernel = f.kernel_release.as_deref().and_then(numeric_start);
    let facts: [(&str, Option<String>); 5] = [("kernel", kernel), ("core", f.core_version.clone()), ("kernel-layer", f.core_version.clone()), ("base", f.release_version.clone()), ("boot", f.release_version.clone())];
    for (comp, fact) in facts {
        // written but not restarted into yet: nothing to judge until the start changes
        if let Some(at) = inst.awaiting.get(comp).cloned() {
            if f.boot_id.as_deref() == Some(at.as_str()) || f.boot_id.is_none() {
                continue;
            }
            inst.awaiting.remove(comp);
        }
        let Some(fact) = fact else { continue };
        match inst.versions.get(comp).cloned() {
            Some(rec) if consistent(&rec, &fact) => {}
            Some(rec) => {
                notes.push(format!("{comp}: the record said {rec} but this system runs {fact}; the record now follows the system."));
                inst.versions.insert(comp.into(), fact);
                inst.shas.remove(comp);
                inst.files.remove(comp);
                inst.how.insert(comp.into(), "observed".into());
            }
            None => {
                inst.versions.insert(comp.into(), fact);
                inst.how.insert(comp.into(), "observed".into());
            }
        }
    }
    if let Some(file) = f.core_file.as_ref().filter(|p| p.is_file()) {
        if inst.versions.contains_key("core") && !inst.files.contains_key("core") {
            inst.files.insert("core".into(), file.clone());
        }
        if hash_core && !inst.shas.contains_key("core") && inst.files.get("core") == Some(file) && !inst.awaiting.contains_key("core") {
            if let Some(h) = sha_of_file(file) {
                inst.shas.insert("core".into(), h);
            }
        }
    }
    notes
}

/// The record for display and planning: read, then corrected from `facts` (not hashed, not saved).
pub fn current(data: &Path, facts: &Facts) -> Installed {
    let mut i = Installed::load(data);
    correct(&mut i, facts, false);
    i
}

/// At boot: read, correct (reading the running program's SHA-256 when it is not on record), and save when something changed.
pub fn correct_at_boot(data: &Path, facts: &Facts) -> Result<Vec<String>, String> {
    let before = Installed::load(data);
    let mut i = before.clone();
    let notes = correct(&mut i, facts, true);
    if i != before || !data.join(INSTALLED_FILE).exists() {
        i.save(data)?;
    }
    Ok(notes)
}

/// One component written by a verified apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied1 {
    pub component: String,
    pub version: String,
    pub sha256: String,
    /// The file it is (for a program of the running system); the boot partition's parts have none.
    pub file: Option<PathBuf>,
    /// The new version only runs after a restart.
    pub needs_restart: bool,
}

/// Record parts written by an apply that has been verified. `boot_id` is the id of the start they were written in. The only caller is
/// the apply step, after its verification.
pub fn record_applied(data: &Path, items: &[Applied1], boot_id: &str) -> Result<Installed, String> {
    let mut i = Installed::load(data);
    for a in items {
        i.versions.insert(a.component.clone(), a.version.clone());
        i.shas.insert(a.component.clone(), a.sha256.clone());
        i.how.insert(a.component.clone(), "applied".into());
        match &a.file {
            Some(f) => {
                i.files.insert(a.component.clone(), f.clone());
            }
            None => {
                i.files.remove(&a.component);
            }
        }
        if a.needs_restart {
            i.awaiting.insert(a.component.clone(), boot_id.to_string());
        } else {
            i.awaiting.remove(&a.component);
        }
    }
    i.save(data)?;
    Ok(i)
}

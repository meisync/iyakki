//! The data science toolkit (F-FLOW-31, docs/development/DATA-SCIENCE-ADDON-PLAN.md): Python with scikit-learn, NumPy, SciPy and OpenCV,
//! a Quick install item that is downloaded from the vendors at the person's click and never shipped in an image.
//!
//! This file is the PIN TABLE and the rules around it. Every file the install may fetch is named here with its exact address and its
//! SHA-256, and nothing else is ever fetched. THE ADDRESSES AND HASHES ARE NOT FILLED IN: the plan lists only the first sixteen hex
//! characters of each hash and cut-off addresses, and a hash is never guessed. Until the owner fills the table (one step, below), the
//! item FAILS CLOSED: [`ready`] says what is missing and the install stops before the first byte is downloaded.
//!
//! OWNER STEP to fill the table: for each row of [`PINS`], download the vendor's file (python-build-standalone release, the wheels on
//! files.pythonhosted.org), compute `sha256sum` of the file, check it against the hash the vendor publishes (PyPI's JSON page, the
//! release's SHA256SUMS), and write the full address and the 64 hex characters into the row. `ready()` then turns true; the table test
//! (`a_filled_table_passes_and_every_problem_is_named`) shows what a good row looks like.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a pinned file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The python-build-standalone `install_only` tree (`.tar.gz`).
    Python,
    /// A wheel (`.whl`, a zip) unpacked into the one `site` folder.
    Wheel,
}

/// One file of the toolkit.
#[derive(Clone, Copy, Debug)]
pub struct Pin {
    pub name: &'static str,
    pub version: &'static str,
    pub kind: Kind,
    /// The complete https address (empty until the owner fills the table).
    pub url: &'static str,
    /// The 64 lowercase hex characters of the SHA-256 of the file (empty until the owner fills the table).
    pub sha256: &'static str,
    /// About how big the file is, megabytes: the size shown before the click and the cap while downloading (with slack).
    pub mb: u32,
    pub licence: &'static str,
}

/// The only hosts a download (and a redirect) may touch. `pypi.org` is not needed: the addresses are pinned.
pub const HOSTS: [&str; 4] = ["files.pythonhosted.org", "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

/// The state of the table as shipped: names, versions, sizes and licences from the plan; addresses and hashes still to be filled.
pub const PINS: [Pin; 9] = [
    Pin { name: "cpython (python-build-standalone install_only)", version: "3.12.15+20261021", kind: Kind::Python, url: "", sha256: "", mb: 64, licence: "PSF-2.0" },
    Pin { name: "numpy", version: "2.5.4", kind: Kind::Wheel, url: "", sha256: "", mb: 17, licence: "BSD-3-Clause and others" },
    Pin { name: "scipy", version: "1.18.1", kind: Kind::Wheel, url: "", sha256: "", mb: 36, licence: "BSD-3-Clause" },
    Pin { name: "scikit-learn", version: "1.9.1", kind: Kind::Wheel, url: "", sha256: "", mb: 10, licence: "BSD-3-Clause" },
    Pin { name: "joblib", version: "1.6.0", kind: Kind::Wheel, url: "", sha256: "", mb: 1, licence: "BSD-3-Clause" },
    Pin { name: "threadpoolctl", version: "3.7.0", kind: Kind::Wheel, url: "", sha256: "", mb: 1, licence: "BSD-3-Clause" },
    Pin { name: "narwhals", version: "2.27.1", kind: Kind::Wheel, url: "", sha256: "", mb: 1, licence: "MIT" },
    Pin { name: "cloudpickle", version: "3.1.2", kind: Kind::Wheel, url: "", sha256: "", mb: 1, licence: "BSD-3-Clause" },
    Pin { name: "opencv-python-headless", version: "4.14.0.94", kind: Kind::Wheel, url: "", sha256: "", mb: 62, licence: "Apache-2.0 with LGPL-2.1 (FFmpeg) and Intel (IPP) parts" },
];

/// Shown at the click, with the "I have read it" confirmation.
pub const LICENCE: &str = "Python is PSF-2.0; scikit-learn, NumPy, SciPy, joblib, threadpoolctl, cloudpickle are BSD-3-Clause, narwhals MIT, OpenCV Apache-2.0. The wheels carry OpenBLAS (BSD), libgfortran and libgomp (GPL-3.0 with the GCC runtime exception), libquadmath (LGPL-2.1+) and, in OpenCV, FFmpeg (LGPL-2.1), libvpx, libaom, libdav1d, libavif (BSD), an old OpenSSL that is never used (no network inside) and, on x86_64, Intel IPP under the Intel Simplified Software License. IYAKKI ships none of it: it downloads the vendors' files unchanged and keeps their licence files in the folder.";

pub const ABOUT: &str = "Python with scikit-learn, NumPy, SciPy and OpenCV for the data and picture flows, downloaded from the vendors at your click and run in a Linux sandbox with no network; nothing of it is in the IYAKKI image";

/// Total download in megabytes (what the Setup tab shows before the click).
pub fn total_mb() -> u32 {
    PINS.iter().map(|p| p.mb).sum()
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.split('@').next_back()?.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

/// Whether a download (or the place a redirect leads) is on the allowed hosts and over https.
pub fn host_allowed(url: &str) -> bool {
    host_of(url).is_some_and(|h| HOSTS.contains(&h))
}

fn file_name(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("")
}

/// Every reason a table cannot be used (empty: it can). One line per reason, naming the row.
pub fn table_problems(pins: &[Pin]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    if !pins.iter().any(|p| p.kind == Kind::Python) {
        out.push("the table has no Python".into());
    }
    for p in pins {
        if !seen.insert((p.name, p.version)) {
            out.push(format!("{} {} is listed twice", p.name, p.version));
        }
        if p.url.is_empty() {
            out.push(format!("{} {}: no address is pinned yet", p.name, p.version));
        } else if !host_allowed(p.url) {
            out.push(format!("{} {}: the address is not an https address on {}", p.name, p.version, HOSTS.join(", ")));
        } else {
            let f = file_name(p.url);
            let ok = match p.kind {
                Kind::Python => f.ends_with(".tar.gz") && f.contains(p.version.split('+').next().unwrap_or("")),
                Kind::Wheel => f.ends_with(".whl") && f.replace('_', "-").to_ascii_lowercase().starts_with(&format!("{}-{}-", p.name.replace('_', "-").to_ascii_lowercase().replace("opencv-python-headless", "opencv-python-headless"), p.version)),
            };
            if !ok {
                out.push(format!("{} {}: the file name {f:?} does not match the name and version", p.name, p.version));
            }
        }
        if p.sha256.is_empty() {
            out.push(format!("{} {}: no SHA-256 is pinned yet", p.name, p.version));
        } else if p.sha256.len() != 64 || !p.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            out.push(format!("{} {}: the SHA-256 is not 64 lowercase hex characters", p.name, p.version));
        }
        if p.mb == 0 {
            out.push(format!("{} {}: no size", p.name, p.version));
        }
    }
    out
}

/// Whether the shipped table can be used. `Err` is the plain sentence the install stops with (before any download).
pub fn ready() -> Result<(), String> {
    check(&PINS)
}

/// [`ready`] for any table (the tests use made-up ones).
pub fn check(pins: &[Pin]) -> Result<(), String> {
    let p = table_problems(pins);
    if p.is_empty() {
        return Ok(());
    }
    Err(format!("The data science toolkit cannot be installed yet: its download table is not complete ({} things to fill in, for example: {}). Nothing was downloaded. See docs/development/DATA-SCIENCE-ADDON-PLAN.md section 3.3 and the owner step in the header of crates/iyakki-shared/src/datasci.rs.", p.len(), p[0]))
}

/// `<data>/datasci`.
pub fn dir(data: &Path) -> PathBuf {
    data.join("datasci")
}

pub fn python_bin(data: &Path) -> PathBuf {
    dir(data).join("python/bin/python3")
}

pub fn site_dir(data: &Path) -> PathBuf {
    dir(data).join("site")
}

/// What is kept after an install (`<data>/datasci/state.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// `name version sha256` of each file that was installed.
    pub files: Vec<String>,
    pub selftest: String,
}

pub fn installed(data: &Path) -> Option<State> {
    let t = std::fs::read_to_string(dir(data).join("state.json")).ok()?;
    let st: State = serde_json::from_str(&t).ok()?;
    (python_bin(data).exists() && site_dir(data).is_dir()).then_some(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: Pin = Pin { name: "numpy", version: "2.5.4", kind: Kind::Wheel, url: "https://files.pythonhosted.org/packages/aa/bb/cc/numpy-2.5.4-cp312-cp312-manylinux_2_28_x86_64.whl", sha256: "fbde6962867ee75bfbde6962867ee75bfbde6962867ee75bfbde6962867ee75b", mb: 17, licence: "BSD" };
    const PY: Pin = Pin { name: "cpython", version: "3.12.15+20261021", kind: Kind::Python, url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261021/cpython-3.12.15+20261021-x86_64-unknown-linux-gnu-install_only.tar.gz", sha256: "5ff9f0602ce964a55ff9f0602ce964a55ff9f0602ce964a55ff9f0602ce964a5", mb: 64, licence: "PSF-2.0" };

    #[test]
    fn the_shipped_table_is_not_filled_in_so_the_item_fails_closed_before_any_download() {
        let e = ready().unwrap_err();
        assert!(e.contains("Nothing was downloaded") && e.contains("no address is pinned yet"), "{e}");
        // every row is named, with the plan's versions
        assert_eq!(table_problems(&PINS).iter().filter(|p| p.contains("no SHA-256")).count(), PINS.len());
        assert_eq!(total_mb(), 64 + 17 + 36 + 10 + 1 + 1 + 1 + 1 + 62);
    }

    #[test]
    fn a_filled_table_passes_and_every_problem_is_named() {
        assert!(check(&[PY, GOOD]).is_ok());
        let mut bad = GOOD;
        bad.url = "http://files.pythonhosted.org/numpy-2.5.4-x.whl";
        assert!(table_problems(&[PY, bad]).iter().any(|p| p.contains("https address")));
        bad.url = "https://evil.example/numpy-2.5.4-cp312.whl";
        assert!(table_problems(&[PY, bad]).iter().any(|p| p.contains("https address")));
        bad.url = "https://files.pythonhosted.org@evil.example/numpy-2.5.4-cp312.whl";
        assert!(!host_allowed(bad.url));
        bad.url = "https://files.pythonhosted.org/packages/x/numpy-2.5.3-cp312.whl";
        assert!(table_problems(&[PY, bad]).iter().any(|p| p.contains("does not match")));
        let mut b2 = GOOD;
        b2.sha256 = "ABC";
        assert!(table_problems(&[PY, b2]).iter().any(|p| p.contains("64 lowercase hex")));
        assert!(table_problems(&[GOOD]).iter().any(|p| p.contains("no Python")));
        assert!(table_problems(&[PY, GOOD, GOOD]).iter().any(|p| p.contains("listed twice")));
    }

    #[test]
    fn nothing_is_installed_without_the_folder_and_the_state() {
        let d = std::env::temp_dir().join(format!("iyakki-datasci-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(installed(&d).is_none());
        std::fs::create_dir_all(dir(&d).join("python/bin")).unwrap();
        std::fs::create_dir_all(site_dir(&d)).unwrap();
        std::fs::write(python_bin(&d), b"").unwrap();
        assert!(installed(&d).is_none(), "no state file yet");
        std::fs::write(dir(&d).join("state.json"), serde_json::to_string(&State { files: vec!["numpy 2.5.4 ab".into()], selftest: "ok".into() }).unwrap()).unwrap();
        assert_eq!(installed(&d).unwrap().selftest, "ok");
        let _ = std::fs::remove_dir_all(&d);
    }
}

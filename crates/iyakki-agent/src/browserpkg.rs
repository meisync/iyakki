//! The browser engine as an OPTIONAL package (F-BROW-40): not in the base image, installed on the person's say-so from the store,
//! with its own declared permissions and a line in the task manager. This is the first-use install logic, tested against a fake store.
//!
//! The package is the engine's helper program (`dist/webview/wpe-helper`, F-BROW-43) as one executable file plus a signed manifest.
//! Install: read the signed manifest (the key is pinned by the caller: the official store key), check size and name, download to a
//! `.part` file with a size cap, check the SHA-256 against the signed manifest, make it executable, put it in its own versioned folder
//! and switch `current` to it LAST. A failed or interrupted install leaves the previous version (or nothing) in place and no part file.
//!
//! NOT here: taking the helper out of the image (a Buildroot change), building and publishing the package, wiring
//! [`installed_helper`] into `webengine::embedded_helper`, and the Store page's Install button. Those are image and screen work.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::store::trust::{sign_doc, verify_doc};

pub const NAME: &str = "iyakki-browser-engine";
pub const PURPOSE: &str = "browserpkg";
/// The most the package may be (the helper is about 90 MB today).
pub const MAX_SIZE: u64 = 400 << 20;
/// What the engine may do, as capability names of the catalogue (`iyakki_core_api::appcaps`): reach the network and play sound. Nothing else:
/// no files outside its own profile, no camera, no microphone unless the person gives it per page (F-BROW-27).
pub const PERMISSIONS: [&str; 2] = ["net.connect", "speaker.play"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub size: u64,
    pub sha256: String,
    /// The architecture of the helper (`x86_64`).
    pub arch: String,
}

pub fn manifest_doc(m: &Manifest, key: &ed25519_dalek::SigningKey) -> Value {
    let mut d = json!({"name": NAME, "version": m.version, "size": m.size, "sha256": m.sha256, "arch": m.arch, "permissions": PERMISSIONS, "key": crate::pkg::public_key_hex(key)});
    let sig = sign_doc(PURPOSE, &d, key);
    d["sig"] = Value::String(sig);
    d
}

fn plain(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
}

/// Read a manifest document and check it against the pinned key; the permissions it declares must be exactly [`PERMISSIONS`].
pub fn read_manifest(doc: &Value, pin: &str, arch: &str) -> Result<Manifest, String> {
    let (key, sig) = (doc["key"].as_str().unwrap_or(""), doc["sig"].as_str().unwrap_or(""));
    let mut unsigned = doc.clone();
    if let Some(o) = unsigned.as_object_mut() {
        o.remove("sig");
    }
    if sig.is_empty() || !key.eq_ignore_ascii_case(pin) || !verify_doc(PURPOSE, &unsigned, key, sig) {
        return Err("the package description is not signed by the store key you trust: refused".into());
    }
    if doc["name"] != NAME {
        return Err("this is not the browser engine package".into());
    }
    let declared: Vec<&str> = doc["permissions"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if declared != PERMISSIONS {
        return Err("the package asks for permissions this version does not allow it: refused".into());
    }
    let m = Manifest { version: doc["version"].as_str().unwrap_or("").into(), size: doc["size"].as_u64().unwrap_or(0), sha256: doc["sha256"].as_str().unwrap_or("").to_ascii_lowercase(), arch: doc["arch"].as_str().unwrap_or("").into() };
    if !plain(&m.version, 40) || m.size == 0 || m.size > MAX_SIZE || m.sha256.len() != 64 || !m.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("the package description is damaged".into());
    }
    if m.arch != arch {
        return Err(format!("the package is for {}, this machine is {arch}", m.arch));
    }
    Ok(m)
}

/// Where the package comes from (the official store, or a person's own; a fake in tests).
pub trait Source {
    fn manifest(&self) -> Result<Value, String>;
    /// Write the package file to `dest` (any size; the caller checks it).
    fn download(&self, dest: &Path) -> Result<(), String>;
}

fn root(data: &Path) -> PathBuf {
    data.join("addons").join("browser")
}

fn current_file(data: &Path) -> PathBuf {
    root(data).join("current")
}

/// What is installed: the version, or None.
pub fn installed_version(data: &Path) -> Option<String> {
    let v = std::fs::read_to_string(current_file(data)).ok()?;
    let v = v.trim();
    (plain(v, 40) && root(data).join(v).join("webview-helper").is_file()).then(|| v.to_string())
}

/// The installed helper program (for `webengine::embedded_helper` to try after the image's own).
pub fn installed_helper(data: &Path) -> Option<PathBuf> {
    installed_version(data).map(|v| root(data).join(v).join("webview-helper"))
}

/// What the Store page shows before the person clicks Install.
#[derive(Debug, PartialEq, Eq)]
pub struct Card {
    pub version: String,
    pub size: u64,
    pub permissions: Vec<&'static str>,
    pub replaces: Option<String>,
    pub lines: Vec<String>,
}

pub fn card(data: &Path, src: &dyn Source, pin: &str, arch: &str) -> Result<Card, String> {
    let m = read_manifest(&src.manifest()?, pin, arch)?;
    let replaces = installed_version(data);
    let mut lines = vec![
        format!("Install the browser engine {} ({} MB) from the store.", m.version, m.size.div_ceil(1 << 20)),
        "It may reach the network and play sound. It gets no other access: not your files, camera or microphone.".to_string(),
        "It runs as its own program, is listed in the task manager, and can be removed at any time.".to_string(),
    ];
    if let Some(r) = &replaces {
        lines.push(if *r == m.version { format!("Version {r} is already installed.") } else { format!("It replaces version {r}, which is kept until the new one is checked.") });
    }
    Ok(Card { version: m.version, size: m.size, permissions: PERMISSIONS.to_vec(), replaces, lines })
}

/// Install, after the person approved the card. Returns the version now current.
pub fn install(data: &Path, src: &dyn Source, pin: &str, arch: &str, approved: bool) -> Result<String, String> {
    if !approved {
        return Err("the person has not approved the install".into());
    }
    let m = read_manifest(&src.manifest()?, pin, arch)?;
    if installed_version(data).as_deref() == Some(m.version.as_str()) {
        return Ok(m.version);
    }
    let dir = root(data).join(&m.version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = dir.join("webview-helper.part");
    let fail = |e: String| {
        let _ = std::fs::remove_file(&part);
        if !dir.join("webview-helper").is_file() {
            let _ = std::fs::remove_dir(&dir);
        }
        Err(e)
    };
    if let Err(e) = src.download(&part) {
        return fail(e);
    }
    let len = std::fs::metadata(&part).map(|x| x.len()).unwrap_or(0);
    if len != m.size {
        return fail(format!("the download is {len} bytes, the signed description says {}: refused", m.size));
    }
    match crate::devmode::files::sha256_file(&part) {
        Ok(h) if h == m.sha256 => {}
        _ => return fail("the download does not match the signed checksum: refused".into()),
    }
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755)) {
            return fail(e.to_string());
        }
    }
    if let Err(e) = std::fs::rename(&part, dir.join("webview-helper")) {
        return fail(e.to_string());
    }
    // switch last, through a temporary file, so a cut leaves the old version current
    let tmp = root(data).join("current.tmp");
    std::fs::write(&tmp, format!("{}\n", m.version)).and_then(|_| std::fs::rename(&tmp, current_file(data))).map_err(|e| e.to_string())?;
    // older versions are removed once the new one is current
    for e in std::fs::read_dir(root(data)).into_iter().flatten().flatten() {
        if e.file_name() != m.version.as_str() && e.path().is_dir() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    Ok(m.version)
}

/// Remove the package (the image's own helper, if any, is not touched).
pub fn remove(data: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(root(data)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// The task manager's line for the package (F-APPS-06): what it is, whether it is installed, which permissions it holds.
#[derive(Debug, PartialEq, Eq)]
pub struct TaskEntry {
    pub name: &'static str,
    pub kind: &'static str,
    pub version: Option<String>,
    pub running: bool,
    pub permissions: Vec<&'static str>,
}

pub fn task_entry(data: &Path, running: bool) -> Option<TaskEntry> {
    installed_version(data).map(|v| TaskEntry { name: NAME, kind: "optional package", version: Some(v), running, permissions: PERMISSIONS.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    struct Fake {
        manifest: Value,
        bytes: Vec<u8>,
        fail: bool,
    }
    impl Source for Fake {
        fn manifest(&self) -> Result<Value, String> {
            Ok(self.manifest.clone())
        }
        fn download(&self, dest: &Path) -> Result<(), String> {
            if self.fail {
                std::fs::write(dest, &self.bytes[..self.bytes.len() / 2]).unwrap();
                return Err("the connection was cut".into());
            }
            std::fs::write(dest, &self.bytes).map_err(|e| e.to_string())
        }
    }

    fn key(n: u8) -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[n; 32])
    }
    fn pin(n: u8) -> String {
        crate::pkg::public_key_hex(&key(n))
    }
    fn fake(version: &str, content: u8, k: u8) -> Fake {
        let bytes = vec![content; 5000];
        let sha = crate::pkg::hex(&Sha256::digest(&bytes));
        Fake { manifest: manifest_doc(&Manifest { version: version.into(), size: 5000, sha256: sha, arch: "x86_64".into() }, &key(k)), bytes, fail: false }
    }
    fn data(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-browserpkg-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn permissions_are_real_catalogue_names_and_nothing_wide() {
        for p in PERMISSIONS {
            assert!(iyakki_core_api::appcaps::lookup(p).is_some(), "{p}");
        }
        assert!(!PERMISSIONS.iter().any(|p| p.starts_with("camera") || p.starts_with("microphone") || p.starts_with("files") || *p == "screen.capture"));
    }

    #[test]
    fn nothing_is_installed_without_the_approval_and_the_card_says_what_it_gets() {
        let d = data("card");
        let f = fake("1.0.0", 1, 1);
        let c = card(&d, &f, &pin(1), "x86_64").unwrap();
        assert!(c.lines[1].contains("network and play sound") && c.lines[1].contains("not your files") && c.replaces.is_none());
        assert!(install(&d, &f, &pin(1), "x86_64", false).is_err());
        assert!(installed_version(&d).is_none() && !root(&d).exists() || std::fs::read_dir(root(&d)).map(|r| r.count()).unwrap_or(0) == 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_good_package_installs_runs_as_an_executable_is_listed_and_can_be_removed() {
        let d = data("ok");
        let f = fake("1.0.0", 7, 1);
        assert_eq!(install(&d, &f, &pin(1), "x86_64", true).unwrap(), "1.0.0");
        let h = installed_helper(&d).unwrap();
        assert!(h.ends_with("addons/browser/1.0.0/webview-helper"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&h).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read(&h).unwrap(), vec![7u8; 5000]);
        let t = task_entry(&d, true).unwrap();
        assert_eq!((t.name, t.kind, t.running, t.permissions.len()), (NAME, "optional package", true, 2));
        assert_eq!(install(&d, &f, &pin(1), "x86_64", true).unwrap(), "1.0.0", "installing the same version again changes nothing");
        remove(&d).unwrap();
        assert!(installed_helper(&d).is_none() && task_entry(&d, false).is_none());
        remove(&d).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_wrong_key_a_changed_description_a_wrong_size_or_hash_and_a_cut_download_leave_nothing() {
        let d = data("bad");
        assert!(install(&d, &fake("1.0.0", 1, 2), &pin(1), "x86_64", true).unwrap_err().contains("not signed by the store key"), "another key");
        let mut tampered = fake("1.0.0", 1, 1);
        tampered.manifest["version"] = json!("9.9.9");
        assert!(install(&d, &tampered, &pin(1), "x86_64", true).is_err(), "a changed field breaks the signature");
        let mut wide = fake("1.0.0", 1, 1);
        let mut doc = wide.manifest.clone();
        doc.as_object_mut().unwrap().remove("sig");
        doc["permissions"] = json!(["net.connect", "speaker.play", "camera.record"]);
        doc["sig"] = Value::String(sign_doc(PURPOSE, &doc.clone().as_object().map(|_| { let mut x = doc.clone(); x.as_object_mut().unwrap().remove("sig"); x }).unwrap(), &key(1)));
        wide.manifest = doc;
        assert!(install(&d, &wide, &pin(1), "x86_64", true).unwrap_err().contains("permissions"), "a package that asks for more is refused even when signed");
        let mut swapped = fake("1.0.0", 1, 1);
        swapped.bytes = vec![2u8; 5000];
        assert!(install(&d, &swapped, &pin(1), "x86_64", true).unwrap_err().contains("checksum"));
        let mut short = fake("1.0.0", 1, 1);
        short.bytes = vec![1u8; 4999];
        assert!(install(&d, &short, &pin(1), "x86_64", true).unwrap_err().contains("bytes"));
        let mut cut = fake("1.0.0", 1, 1);
        cut.fail = true;
        assert!(install(&d, &cut, &pin(1), "x86_64", true).is_err());
        assert!(install(&d, &fake("1.0.0", 1, 1), &pin(1), "aarch64", true).unwrap_err().contains("is for x86_64"));
        assert!(installed_version(&d).is_none());
        assert!(std::fs::read_dir(root(&d)).map(|r| r.count()).unwrap_or(0) == 0, "no folder, no part file left behind");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_update_keeps_the_old_version_until_the_new_one_is_current_and_a_failed_update_keeps_it_for_good() {
        let d = data("upd");
        install(&d, &fake("1.0.0", 1, 1), &pin(1), "x86_64", true).unwrap();
        let mut broken = fake("1.1.0", 2, 1);
        broken.bytes = vec![3u8; 5000];
        assert!(install(&d, &broken, &pin(1), "x86_64", true).is_err());
        assert_eq!(installed_version(&d).as_deref(), Some("1.0.0"), "the working version stays");
        let c = card(&d, &fake("1.1.0", 2, 1), &pin(1), "x86_64").unwrap();
        assert_eq!(c.replaces.as_deref(), Some("1.0.0"));
        install(&d, &fake("1.1.0", 2, 1), &pin(1), "x86_64", true).unwrap();
        assert_eq!(installed_version(&d).as_deref(), Some("1.1.0"));
        assert!(!root(&d).join("1.0.0").exists(), "the old version goes once the new one is current");
        let _ = std::fs::remove_dir_all(&d);
    }
}

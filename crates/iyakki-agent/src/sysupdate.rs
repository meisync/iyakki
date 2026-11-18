//! The system update bundle (F-UPD-01): ONE signed `.tgz` that carries a system update, for an update channel, a private address, or a
//! file on a USB stick for a machine that is offline.
//!
//! * The archive holds `release.json` (version, variant, minimum base version, release notes, the files with sizes and SHA-256 sums)
//!   and the files themselves under `boot/` and `EFI/` (the same layout as an update pack of `iyakki dev update-pack`).
//! * A detached signature file next to it (`<bundle>.sig`, 128 hex characters) is an ed25519 signature over a line that names the SHA-256
//!   of the whole archive; it is checked with the release key BEFORE anything is unpacked.
//! * [`open`] checks the signature, unpacks into an empty staging folder, checks every listed file and refuses any file that is not
//!   listed, then writes the `update.json` that the existing update flow (`devmode::update`: plan, previous-version copy, apply at the
//!   next restart) already reads. It never applies anything and never restarts: applying stays the person's approval.
//!
//! The channel address and the official release key wait for the owner (F-UPD-01 Notes): [`open`] takes the key as a parameter.

use std::io::Read;
use std::path::{Path, PathBuf};

use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::devmode::update as up;

pub const RELEASE_FILE: &str = "release.json";
/// The signature covers this line: a fixed name for the purpose, then the SHA-256 of the archive.
const PURPOSE: &str = "iyakki-system-update-bundle-1";
/// An archive larger than this is not a system update.
pub const MAX_BUNDLE: u64 = (up::MAX_TOTAL) + (64 << 20);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseJson {
    #[serde(default = "one")]
    pub schema: u32,
    pub version: String,
    pub variant: String,
    /// The oldest installed version this update may be applied on top of.
    #[serde(default)]
    pub min_base: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub built: String,
    pub files: Vec<ReleaseFile>,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, Value>,
}

fn one() -> u32 {
    1
}

/// What a checked bundle gives back.
#[derive(Debug, Clone)]
pub struct Staged {
    pub dir: PathBuf,
    pub release: ReleaseJson,
    pub manifest: up::Manifest,
}

fn signed_line(sha_hex: &str) -> Vec<u8> {
    format!("{PURPOSE}\n{sha_hex}\n").into_bytes()
}

fn sha_of_file(p: &Path) -> Result<(String, u64), String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let (mut h, mut n, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 20]);
    loop {
        let k = f.read(&mut buf).map_err(|e| e.to_string())?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((h.finalize().iter().map(|b| format!("{b:02x}")).collect(), n))
}

/// The owner's tool: the signature text for a bundle file.
pub fn sign(bundle: &Path, key: &SigningKey) -> Result<String, String> {
    let (sha, _) = sha_of_file(bundle)?;
    Ok(crate::dlmanifest::sign_bytes(key, &signed_line(&sha)))
}

/// The owner's tool: make the bundle file from a folder that holds `release.json` and the files it lists (`boot/...`, `EFI/...`). The
/// archive is a plain tar in gzip "stored" blocks (no compression: the kernel and the initrd are compressed already), with fixed owner
/// and time so the same folder gives the same bytes. Sign it with [`sign`].
pub fn pack(folder: &Path, out: &Path) -> Result<(), String> {
    let mut names = Vec::new();
    list_files(folder, "", &mut names)?;
    names.sort();
    if !names.iter().any(|n| n == RELEASE_FILE) {
        return Err(format!("{} has no release.json", folder.display()));
    }
    let mut t = Vec::new();
    for name in &names {
        let body = std::fs::read(folder.join(name)).map_err(|e| format!("{name}: {e}"))?;
        if name.len() > 99 || name.starts_with('/') || name.split('/').any(|c| c == ".." || c.is_empty()) {
            return Err(format!("{name} is not a name an update may carry"));
        }
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000644\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        t.extend_from_slice(&h);
        t.extend_from_slice(&body);
        t.extend(std::iter::repeat_n(0u8, body.len().div_ceil(512) * 512 - body.len()));
    }
    t.extend(std::iter::repeat_n(0u8, 1024));
    let mut g = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    for (i, c) in t.chunks(60000).enumerate() {
        let last = (i + 1) * 60000 >= t.len();
        g.push(u8::from(last));
        g.extend_from_slice(&(c.len() as u16).to_le_bytes());
        g.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        g.extend_from_slice(c);
    }
    g.extend_from_slice(&iyakki_archive::xz::crc32(&t).to_le_bytes());
    g.extend_from_slice(&(t.len() as u32).to_le_bytes());
    std::fs::write(out, g).map_err(|e| format!("{}: {e}", out.display()))
}

fn list_files(base: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), String> {
    for e in std::fs::read_dir(base.join(rel)).map_err(|e| e.to_string())?.flatten() {
        let r = if rel.is_empty() { e.file_name().to_string_lossy().into_owned() } else { format!("{rel}/{}", e.file_name().to_string_lossy()) };
        let t = e.file_type().map_err(|e| e.to_string())?;
        if t.is_dir() {
            list_files(base, &r, out)?;
        } else if t.is_file() {
            out.push(r);
        } else {
            return Err(format!("{r}: a link or special file is not allowed in an update"));
        }
    }
    Ok(())
}

/// Check the signature, unpack into `stage` (an empty or new folder), check every file and write `update.json` there. `scratch` holds the
/// uncompressed tar while it is read (it needs as much room as the update) and is cleaned up afterwards.
pub fn open(bundle: &Path, sig_text: &str, key: &VerifyingKey, scratch: &Path, stage: &Path) -> Result<Staged, String> {
    let (sha, size) = sha_of_file(bundle)?;
    if size > MAX_BUNDLE {
        return Err(format!("this file is {} MB: more than a system update carries", size >> 20));
    }
    crate::dlmanifest::verify_signature(key, &signed_line(&sha), sig_text).map_err(|_| "the signature does not match this update file and the IYAKKI release key: it was changed or is not from IYAKKI. Nothing was unpacked.".to_string())?;
    if stage.exists() && std::fs::read_dir(stage).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} is not empty", stage.display()));
    }
    std::fs::create_dir_all(stage).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let res = (|| {
        iyakki_archive::archive::untar_gz(bundle, stage, scratch).map_err(|e| format!("the update could not be unpacked: {e}"))?;
        check_staged(stage)
    })();
    if res.is_err() {
        // what was unpacked from a bad bundle is moved out of the way, never installed
        let _ = std::fs::remove_dir_all(stage);
    }
    res
}

fn check_staged(stage: &Path) -> Result<Staged, String> {
    let rj = std::fs::read(stage.join(RELEASE_FILE)).map_err(|_| "the update has no release.json".to_string())?;
    let release: ReleaseJson = serde_json::from_slice(&rj).map_err(|e| format!("release.json: {e}"))?;
    if release.schema != 1 {
        return Err(format!("release.json schema {} is not understood by this IYAKKI; update to a newer IYAKKI first", release.schema));
    }
    let mut on_disk = vec![];
    list_files(stage, "", &mut on_disk)?;
    on_disk.retain(|p| p != RELEASE_FILE);
    for p in &on_disk {
        if !release.files.iter().any(|f| &f.path == p) {
            return Err(format!("{p} is in the update but release.json does not list it"));
        }
    }
    let manifest = up::Manifest {
        format: up::FORMAT,
        version: release.version.clone(),
        variant: release.variant.clone(),
        built: release.built.clone(),
        source: "system update bundle".into(),
        secure_boot: release.files.iter().any(|f| matches!(f.path.to_ascii_lowercase().as_str(), "efi/boot/mmx64.efi" | "efi/boot/grubx64.efi")),
        files: release.files.iter().map(|f| up::FileEntry { path: f.path.clone(), size: f.size, sha256: f.sha256.clone() }).collect(),
        layers: None,
    };
    up::verify_payload(stage, &manifest)?;
    up::write_manifest(stage, &manifest)?;
    Ok(Staged { dir: stage.to_path_buf(), release, manifest })
}

/// Where checked bundles are unpacked for the update centre (it lists them like any update folder).
pub fn staged_root(data: &Path) -> PathBuf {
    data.join("updates").join("bundles")
}

/// The whole hand-over of a bundle file (a stick or a download) to the update centre: the signature file `<bundle>.sig` next to it is
/// checked with the release key before anything is unpacked, the unpacked update is written under [`staged_root`] where the centre finds
/// it, and an update that does not fit this computer (another edition, too old a base, not newer) is removed again with the reason.
/// Nothing is applied: installing stays a click of the person on Settings -> System update.
pub fn open_for_centre(data: &Path, bundle: &Path, key: Option<&VerifyingKey>, running_version: &str, running_variant: &str) -> Result<Staged, String> {
    let key = key.ok_or("this computer has no IYAKKI release key, so a signed update file cannot be checked and is not opened")?;
    if !bundle.is_file() {
        return Err(format!("{} is not a file", bundle.display()));
    }
    let sig_path = PathBuf::from(format!("{}.sig", bundle.display()));
    let sig = std::fs::read_to_string(&sig_path).map_err(|_| format!("the signature file {} is missing: a system update is opened only with its .sig file next to it", sig_path.display()))?;
    let root = staged_root(data);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let (sha, _) = sha_of_file(bundle)?;
    let stage = root.join(format!("bundle-{}", &sha[..16]));
    if stage.exists() {
        std::fs::remove_dir_all(&stage).map_err(|e| e.to_string())?;
    }
    let scratch = data.join("updates").join("scratch");
    let r = open(bundle, sig.trim(), key, &scratch, &stage);
    let _ = std::fs::remove_dir_all(&scratch);
    let staged = r?;
    if let Err(why) = applicable(&staged, running_version, running_variant) {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(why);
    }
    Ok(staged)
}

/// Read one answer of an update channel into `dest`, at most `cap` bytes: https only, at most 5 redirects (each one also https), a status
/// other than 200 or a body over the cap is refused and nothing stays on disk.
fn fetch_file(st: &dyn iyakki_proxy::Streamer, url: &str, dest: &Path, cap: u64) -> Result<(), String> {
    let mut url = url.to_string();
    for _ in 0..6 {
        if !url.starts_with("https://") || url.contains(['\0', ' ', '\n']) {
            return Err(format!("an update is fetched over https only; {url} is not"));
        }
        let head = st.open(&url, &[]).map_err(|e| format!("could not reach {url}: {e}"))?;
        if (300..400).contains(&head.status) {
            let loc = head.header("location").ok_or("a redirect without a place to go")?.to_string();
            url = if loc.starts_with('/') { format!("https://{}{loc}", url.trim_start_matches("https://").split('/').next().unwrap_or("")) } else { loc };
            continue;
        }
        if head.status != 200 {
            return Err(format!("{url} answered HTTP {}", head.status));
        }
        if head.header("content-length").and_then(|v| v.parse::<u64>().ok()).is_some_and(|n| n > cap) {
            return Err(format!("the download is larger than the {} MB an update may be; refused", cap >> 20));
        }
        let part = dest.with_extension("part");
        let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let r = std::io::copy(&mut head.body.take(cap + 1), &mut out).map_err(|e| format!("the download broke off: {e}"));
        drop(out);
        match r {
            Ok(n) if n <= cap => return std::fs::rename(&part, dest).map_err(|e| e.to_string()),
            Ok(_) => {
                let _ = std::fs::remove_file(&part);
                return Err(format!("the download is larger than the {} MB an update may be; stopped", cap >> 20));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        }
    }
    Err("too many redirects".into())
}

/// Fetch a system update from an update channel address (`https://.../iyakki-X.tgz`, its signature at the same address with `.sig` added)
/// and hand it to the update centre like a file from a stick ([`open_for_centre`]): the signature is checked with the release key before
/// anything is unpacked, the downloaded files are removed afterwards, nothing is installed. The caller supplies the transport (it holds the
/// account's host rules and the person's approval); the address is the person's, the official channel address waits for the owner.
pub fn fetch_for_centre(data: &Path, st: &dyn iyakki_proxy::Streamer, url: &str, key: Option<&VerifyingKey>, running_version: &str, running_variant: &str) -> Result<Staged, String> {
    if key.is_none() {
        return Err("this computer has no IYAKKI release key, so a signed update cannot be checked and is not downloaded".into());
    }
    let path = url;
    if path.contains(['?', '#']) {
        return Err("a system update address has no query or fragment (its signature is the same address with .sig added)".into());
    }
    if !(path.ends_with(".tgz") || path.ends_with(".tar.gz")) {
        return Err("a system update address ends in .tgz".into());
    }
    let dir = data.join("updates").join("download");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let bundle = dir.join("update.tgz");
    let r = (|| {
        fetch_file(st, &format!("{path}.sig"), &dir.join("update.tgz.sig"), 4096)?;
        fetch_file(st, url, &bundle, MAX_BUNDLE)?;
        open_for_centre(data, &bundle, key, running_version, running_variant)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    r
}

/// May this bundle be applied on a computer that runs `running_version` of `running_variant`? One plain sentence when not.
pub fn applicable(s: &Staged, running_version: &str, running_variant: &str) -> Result<(), String> {
    if !up::same_variant(&s.release.variant, running_variant) {
        return Err(format!("this update is for the {} edition; this computer runs {running_variant}", s.release.variant));
    }
    if !s.release.min_base.is_empty() && up::compare_versions(running_version, &s.release.min_base) == std::cmp::Ordering::Less {
        return Err(format!("this update needs IYAKKI {} or newer first; this computer has {running_version}. Apply the earlier update first.", s.release.min_base));
    }
    if up::compare_versions(&s.release.version, running_version) != std::cmp::Ordering::Greater {
        return Err(format!("this update is version {}, which is not newer than the {running_version} that runs here", s.release.version));
    }
    Ok(())
}

#[cfg(test)]
#[path = "sysupdate_tests.rs"]
mod tests;

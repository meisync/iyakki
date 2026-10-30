//! The engine of the Update centre (Settings -> System update): what a person without a developer computer does with an update pack
//! they were given on a stick or in a folder. It is a thin, checked layer over [`super::update`] (the same staging, the same previous
//! version kept on the data partition, the same boot menu entry "IYAKKI (previous version)"); it adds:
//!
//! * finding packs in a folder the person chooses (the folder itself, its sub-folders, two levels);
//! * trust: `update.json.sig` is a detached ed25519 signature over the exact bytes of `update.json`, checked with the key that
//!   already signs the downloads manifest ([`crate::dlmanifest`]). A bad signature is always refused; a pack with no signature
//!   only installs when the person confirms. The per-file SHA-256 in `update.json` then cover every payload file;
//! * a plain preview (what changes, what needs a confirmation: another variant, an older version, no signature);
//! * install from the pack folder (every file is hashed BEFORE anything is written; only the staging folder on the data partition
//!   and the device's own boot partition are written, never another drive; the device never restarts by itself);
//! * rollback: the version saved by the last update becomes the one on the boot partition (the engine's own update, forced to the
//!   older version, which saves the version being left in turn, so rolling forward is possible);
//! * a check after the next start (`after_boot`): the new version is marked good when it runs, or the fall-back is recorded.

use std::path::{Path, PathBuf};

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::files;
use super::update as up;
use super::DevMode;

pub const SIG_FILE: &str = "update.json.sig";
pub const TRIAL_FILE: &str = "update-trial.json";
/// A pack's manifest is small; anything bigger is not one.
const MAX_MANIFEST: u64 = 4 << 20;
const MAX_SCAN_DIRS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// `update.json.sig` matches `update.json` and the release key.
    Signed,
    /// No signature (or this build has no release key to check it with).
    Unsigned,
    /// A signature that does not match: never installed.
    Bad(String),
}

#[derive(Debug, Clone)]
pub struct Found {
    pub dir: PathBuf,
    pub manifest: up::Manifest,
    pub trust: Trust,
}

/// Read the pack in `dir`: manifest checked on its own terms, signature checked when there is one.
pub fn read_pack(dir: &Path, key: Option<&VerifyingKey>) -> Result<Found, String> {
    let mp = dir.join(up::MANIFEST);
    let meta = std::fs::symlink_metadata(&mp).map_err(|e| format!("{}: {e}", mp.display()))?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST {
        return Err(format!("{} is not an update description", mp.display()));
    }
    let bytes = std::fs::read(&mp).map_err(|e| e.to_string())?;
    let manifest: up::Manifest = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", up::MANIFEST))?;
    up::check_manifest(&manifest)?;
    let trust = match (std::fs::read_to_string(dir.join(SIG_FILE)), key) {
        (Ok(sig), Some(k)) => match crate::dlmanifest::verify_signature(k, &bytes, &sig) {
            Ok(()) => Trust::Signed,
            Err(e) => Trust::Bad(e),
        },
        _ => Trust::Unsigned,
    };
    Ok(Found { dir: dir.to_path_buf(), manifest, trust })
}

/// Sign a pack folder (the owner's tool; the key is read by the caller from a path given at run time).
pub fn sign_pack(dir: &Path, key: &ed25519_dalek::SigningKey) -> Result<(), String> {
    let bytes = std::fs::read(dir.join(up::MANIFEST)).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(SIG_FILE), crate::dlmanifest::sign_bytes(key, &bytes)).map_err(|e| e.to_string())
}

/// Packs in `root` itself and in its sub-folders down to two levels (a stick with `iyakki-update/0.2.0/`). Symbolic links are not followed.
pub fn find_packs(root: &Path, key: Option<&VerifyingKey>) -> Vec<Found> {
    let mut out = Vec::new();
    let mut seen = 0usize;
    fn walk(dir: &Path, depth: u8, key: Option<&VerifyingKey>, out: &mut Vec<Found>, seen: &mut usize) {
        if *seen >= MAX_SCAN_DIRS {
            return;
        }
        *seen += 1;
        if dir.join(up::MANIFEST).is_file() {
            if let Ok(f) = read_pack(dir, key) {
                out.push(f);
            }
            return;
        }
        if depth == 0 {
            return;
        }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
        subs.sort();
        for s in subs {
            walk(&s, depth - 1, key, out, seen);
        }
    }
    walk(root, 2, key, &mut out, &mut seen);
    out.sort_by(|a, b| up::compare_versions(&b.manifest.version, &a.manifest.version).then(a.dir.cmp(&b.dir)));
    out
}

// -------------------------------------------------------------------------------------------- the trial record

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    pub from: String,
    pub to: String,
    pub applied: i64,
    /// The kernel's boot id when the update was written: the same id means the device has not restarted yet.
    pub boot_id: String,
    /// `restart` (waiting for the next start), `good`, `fell-back`.
    pub state: String,
    /// `update` or `rollback`.
    pub kind: String,
}

pub fn read_trial(data: &Path) -> Option<Trial> {
    serde_json::from_slice(&std::fs::read(data.join(TRIAL_FILE)).ok()?).ok()
}

fn write_trial(data: &Path, t: &Trial) -> Result<(), String> {
    let tmp = data.join(format!("{TRIAL_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(t).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, data.join(TRIAL_FILE)).map_err(|e| e.to_string())
}

/// The kernel's id of this boot (empty when it cannot be read).
pub fn boot_id_now() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_string()).unwrap_or_default()
}

/// What the check after a start found, in words for the person; `None` when there is nothing to say.
/// `running` is the version that runs now, `boot_id` this start's id.
pub fn after_boot(data: &Path, running: &str, boot_id: &str) -> Option<String> {
    let mut t = read_trial(data)?;
    if t.state != "restart" {
        return None;
    }
    // same start as the one the update was written in: nothing has been tried yet
    if !boot_id.is_empty() && boot_id == t.boot_id {
        return None;
    }
    let what = if t.kind == "rollback" { "The previous version" } else { "The new version" };
    if up::compare_versions(running, &t.to) == std::cmp::Ordering::Equal {
        t.state = "good".into();
        let _ = write_trial(data, &t);
        Some(format!("{what} ({}) started and is marked good. The version before it ({}) stays saved for a rollback.", t.to, t.from))
    } else {
        t.state = "fell-back".into();
        let _ = write_trial(data, &t);
        Some(format!("IYAKKI {} was installed but this start is {running}: the boot menu fell back to the saved version. Nothing was lost; open System update to see what to do.", t.to))
    }
}

// -------------------------------------------------------------------------------------------- the centre

pub struct Centre {
    dm: DevMode,
    boot: Box<dyn up::BootMount>,
    release: String,
    volatile: bool,
    key: Option<VerifyingKey>,
    data_free: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Versions {
    /// The version running now, and the version whose files are on the boot partition (differs after an update, before the restart).
    pub running: String,
    pub on_disk: String,
    pub variant: String,
    pub previous: Option<String>,
    /// A sentence when an update was interrupted.
    pub interrupted: Option<String>,
    pub trial: Option<Trial>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub from: String,
    pub to: String,
    pub variant: String,
    pub trust: Trust,
    /// Files that will be written, files already the same, bytes that will be written.
    pub changes: usize,
    pub same: usize,
    pub bytes: u64,
    /// Reasons that need the person's yes: other variant, older version, no signature.
    pub confirm: Vec<String>,
    pub notes: Vec<String>,
}

impl Preview {
    /// Plain lines for the page.
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![
            format!("IYAKKI {} -> {} ({})", self.from, self.to, self.variant),
            format!("{} file(s) on the device's boot partition will be replaced ({} MB), {} stay as they are.", self.changes, self.bytes.div_ceil(1 << 20), self.same),
            "Your files, accounts and settings are not touched.".into(),
            format!("The version you have now ({}) is kept and can be started from the boot menu or rolled back to.", self.from),
        ];
        v.push(match &self.trust {
            Trust::Signed => "Signed: the signature matches the IYAKKI release key.".into(),
            Trust::Unsigned => "Not signed: only the file checksums in the pack are checked.".into(),
            Trust::Bad(e) => format!("The signature is wrong: {e}"),
        });
        v.extend(self.confirm.iter().map(|c| format!("Needs your yes: {c}")));
        v
    }
}

impl Centre {
    pub fn new(data: &Path, boot: Box<dyn up::BootMount>, release: String, volatile: bool, key: Option<VerifyingKey>) -> Centre {
        Centre { dm: DevMode::open(data), boot, release, volatile, key, data_free: None }
    }

    /// The device: the boot partition found on the data volume's own disk, `/etc/iyakki-release`, the built-in release key.
    pub fn real(data: &Path) -> Centre {
        let mounted = std::fs::read_to_string("/proc/mounts").map(|t| super::bootmount::parse_mounts(&t).iter().any(|m| m.dir == data)).unwrap_or(false);
        Centre::new(data, Box::new(super::bootmount::SysBoot::real(data)), std::fs::read_to_string(crate::relinfo::RELEASE_FILE).unwrap_or_default(), !mounted, crate::dlmanifest::builtin_key())
    }

    /// Fix the free space of the data partition (tests).
    pub fn with_data_free(mut self, n: u64) -> Centre {
        self.data_free = Some(n);
        self
    }

    pub fn key(&self) -> Option<&VerifyingKey> {
        self.key.as_ref()
    }

    pub fn data(&self) -> &Path {
        self.dm.data_dir()
    }

    fn with_env<T>(&self, f: impl FnOnce(&up::Env) -> T) -> T {
        let data = self.dm.data_dir().to_path_buf();
        let free = || self.data_free.unwrap_or_else(|| super::debug::statvfs(&data).1);
        let env = up::Env { dm: &self.dm, boot: self.boot.as_ref(), release: self.release.clone(), volatile: self.volatile, data_free: &free, fault_after: None };
        f(&env)
    }

    pub fn versions(&self) -> Versions {
        let data = self.dm.data_dir();
        let st = up::status(data);
        let (running, on_disk, variant) = match self.boot.open(false) {
            Ok(vol) => {
                let r = up::running_from(&self.release, &vol.root);
                (r.version.clone(), r.on_disk().to_string(), r.variant)
            }
            Err(_) => {
                let r = crate::relinfo::ReleaseInfo::parse(&self.release);
                let v = if r.version.is_empty() { crate::relinfo::VERSION.to_string() } else { r.version };
                (v.clone(), v, r.variant)
            }
        };
        Versions { running, on_disk, variant, previous: st["previous_version"].as_str().map(str::to_string), interrupted: st["message"].as_str().map(str::to_string), trial: read_trial(data) }
    }

    /// Everything that can be checked before a byte moves; the answer says what needs the person's yes. `Err` is a refusal no yes can lift.
    pub fn preview(&self, f: &Found) -> Result<Preview, String> {
        if let Trust::Bad(e) = &f.trust {
            return Err(format!("the signature of this update does not match, so it is refused: {e}"));
        }
        let m = &f.manifest;
        let mut confirm = Vec::new();
        let p = self.with_env(|env| match up::plan(env, m, false) {
            Ok(p) => Ok(p),
            Err(first) => match up::plan(env, m, true) {
                Ok(p) => {
                    confirm.push(first);
                    Ok(p)
                }
                Err(hard) => Err(hard),
            },
        })?;
        if f.trust == Trust::Unsigned {
            confirm.push("this update has no signature, so nothing proves who made it".into());
        }
        let bytes = p.needs.iter().filter_map(|n| m.files.iter().find(|f| &f.path == n)).map(|f| f.size).sum();
        Ok(Preview { from: p.from, to: p.to, variant: m.variant.clone(), trust: f.trust.clone(), changes: p.needs.len(), same: p.same, bytes, confirm, notes: p.notes })
    }

    /// Install. `confirmed` is the person's yes to every line in `Preview::confirm`. Does not restart.
    pub fn install(&self, f: &Found, confirmed: bool, who: &str) -> Result<String, String> {
        let pv = self.preview(f)?;
        if !pv.confirm.is_empty() && !confirmed {
            return Err(format!("this needs your confirmation: {}", pv.confirm.join("; ")));
        }
        // the pack is read again: a stick can change between the preview and the click
        let again = read_pack(&f.dir, self.key.as_ref())?;
        if again.manifest != f.manifest || again.trust != f.trust {
            return Err("the update on the drive changed since it was shown: look at it again".into());
        }
        // every file against its SHA-256 before anything is written
        up::verify_payload(&f.dir, &f.manifest)?;
        self.with_env(|env| {
            let p = up::plan(env, &f.manifest, confirmed)?;
            let staged = self.stage(env, &f.manifest, &p.needs, &|rel| f.dir.join(rel));
            if let Err(e) = staged {
                self.clean_staging();
                return Err(e);
            }
            let boot_before = boot_id_now();
            match up::apply(env, &f.manifest, confirmed, who) {
                Ok(v) => {
                    let _ = write_trial(self.dm.data_dir(), &Trial { from: p.from.clone(), to: f.manifest.version.clone(), applied: self.dm.now(), boot_id: boot_before, state: "restart".into(), kind: "update".into() });
                    Ok(format!("{} Restart when you are ready to use IYAKKI {}; the device does not restart by itself.", v["message"].as_str().unwrap_or("Done."), f.manifest.version))
                }
                Err(e) => {
                    self.clean_staging();
                    Err(e)
                }
            }
        })
    }

    /// Copy the files in `needs` into `boot-next/`, each checked against the manifest's SHA-256 as it lands.
    fn stage(&self, env: &up::Env, m: &up::Manifest, needs: &[String], src: &dyn Fn(&str) -> PathBuf) -> Result<(), String> {
        let st = self.dm.data_dir().join("boot-next");
        std::fs::create_dir_all(&st).map_err(|e| format!("{}: {e}", st.display()))?;
        for entry in std::fs::read_dir(&st).into_iter().flatten().flatten() {
            if entry.file_name() == "UPDATING.json" {
                continue;
            }
            let p = entry.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
        std::fs::write(st.join(up::MANIFEST), serde_json::to_vec_pretty(m).unwrap_or_default()).map_err(|e| e.to_string())?;
        for n in needs {
            let f = m.files.iter().find(|f| &f.path == n).ok_or("internal: file missing from the manifest")?;
            let from = src(n);
            let to = st.join(n);
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&from, &to).map_err(|e| format!("{n}: {e}"))?;
            std::fs::File::open(&to).and_then(|h| h.sync_all()).map_err(|e| e.to_string())?;
            if files::sha256_file(&to)? != f.sha256 {
                return Err(format!("{n} was damaged while it was copied to the device: nothing was changed"));
            }
        }
        let _ = env;
        Ok(())
    }

    /// Remove the staged files after a failure, but never the marker of an interrupted update.
    fn clean_staging(&self) {
        let st = self.dm.data_dir().join("boot-next");
        for entry in std::fs::read_dir(&st).into_iter().flatten().flatten() {
            if entry.file_name() == "UPDATING.json" {
                continue;
            }
            let p = entry.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }

    fn prev_dir(&self) -> Option<(String, String, PathBuf)> {
        let t = std::fs::read_to_string(self.dm.data_dir().join("boot-prev").join("latest")).ok()?;
        let (id, ver) = t.trim().split_once('\n')?;
        let d = self.dm.data_dir().join("boot-prev").join(id);
        (d.join("bzImage").is_file() && d.join("initrd").is_file()).then(|| (id.to_string(), ver.to_string(), d))
    }

    /// Put the saved previous version back on the boot partition. Same checks, same marker and same previous-version entry as an
    /// update; the version being left is saved in turn. Does not restart.
    pub fn rollback(&self, who: &str) -> Result<String, String> {
        if let Some(t) = up::interrupted_text(self.dm.data_dir()) {
            return Err(t);
        }
        let (pid, pver, pdir) = self.prev_dir().ok_or("no previous version is kept on this device: there is nothing to roll back to")?;
        let meta: Value = std::fs::read(pdir.join("prev.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let (cfg_now, run) = {
            let vol = self.boot.open(false)?;
            let run = up::running_from(&self.release, &vol.root);
            let cfg = up::find_ci(&vol.root, "boot/grub/grub.cfg").and_then(|p| std::fs::read(p).ok()).ok_or("the boot menu is missing from the boot partition")?;
            (cfg, run)
        };
        if run.on_disk() == pver {
            return Err(format!("the boot partition already holds {pver}"));
        }
        // the small files of the payload are made beside the staging folder; the kernel and initramfs are read from boot-prev
        let small = self.dm.data_dir().join("boot-rollback");
        let _ = std::fs::remove_dir_all(&small);
        let rel_text = format!("VERSION={pver}\nVARIANT={}\nBUILD_DATE={}\n", meta["variant"].as_str().filter(|v| !v.is_empty()).unwrap_or(&run.variant), meta["built"].as_str().unwrap_or(""));
        std::fs::create_dir_all(small.join("boot/grub")).map_err(|e| e.to_string())?;
        std::fs::write(small.join("boot/grub/grub.cfg"), &cfg_now).map_err(|e| e.to_string())?;
        std::fs::write(small.join("boot/iyakki-release"), &rel_text).map_err(|e| e.to_string())?;
        let src = |rel: &str| -> PathBuf {
            match rel {
                "boot/bzImage" => pdir.join("bzImage"),
                "boot/initrd" => pdir.join("initrd"),
                other => small.join(other),
            }
        };
        let mut list = Vec::new();
        for rel in ["boot/bzImage", "boot/initrd", "boot/grub/grub.cfg", "boot/iyakki-release"] {
            let p = src(rel);
            let size = std::fs::metadata(&p).map_err(|e| format!("{rel}: {e}"))?.len();
            list.push((rel.to_string(), size, files::sha256_file(&p)?));
        }
        let mut m = up::build_manifest(&small, &list, Some(&pver), Some(&run.variant), "rollback")?;
        m.secure_boot = run.secure_boot;
        let result = self.with_env(|env| {
            // an older version on purpose: the person asked for it
            let p = up::plan(env, &m, true)?;
            self.stage(env, &m, &p.needs, &src).inspect_err(|_| self.clean_staging())?;
            let boot_before = boot_id_now();
            // while the files are replaced, the menu's "previous version" entry is the version being restored (which stays saved)
            let v = up::apply_with_entry(env, &m, true, who, Some((pid.clone(), pver.clone()))).inspect_err(|_| self.clean_staging())?;
            let _ = write_trial(self.dm.data_dir(), &Trial { from: p.from.clone(), to: pver.clone(), applied: self.dm.now(), boot_id: boot_before, state: "restart".into(), kind: "rollback".into() });
            Ok::<_, String>((p.from, v))
        });
        let _ = std::fs::remove_dir_all(&small);
        let (left, _v) = result?;
        // finished: the entry now offers the version just left (to go forward again)
        self.point_entry_at_latest();
        Ok(format!("IYAKKI {pver} is back on the boot partition (the version {left} is kept). Restart when you are ready; the device does not restart by itself."))
    }

    /// Rewrite the previous-version entry of the boot menu to the newest saved copy (a tiny file, replaced atomically). Best effort.
    fn point_entry_at_latest(&self) {
        let Some((id, ver, _)) = self.prev_dir() else { return };
        let Ok(vol) = self.boot.open(true) else { return };
        let Some(p) = up::find_ci(&vol.root, "boot/grub/grub.cfg") else { return };
        let Ok(cfg) = std::fs::read_to_string(&p) else { return };
        let Ok(new) = up::with_previous_entry(&cfg, &id, &ver) else { return };
        let tmp = p.with_file_name("grub.cfg.upd");
        if std::fs::write(&tmp, new.as_bytes()).is_ok() && std::fs::File::open(&tmp).and_then(|f| f.sync_all()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

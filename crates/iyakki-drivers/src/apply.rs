//! Putting a driver pack on the machine: files on the data volume, a symlink farm that makes the kernel find the
//! firmware, modprobe options, and (only when the kernel release matches and the signature chain was good) prebuilt
//! modules. Nothing is compiled here, ever.
//!
//! Layout, with `<data>` the data volume (`/var/lib/iyakki`):
//!
//! ```text
//! <data>/firmware/<pack>/pack.json          what the pack asked for (no blobs)
//! <data>/firmware/<pack>/files/<path>       the firmware blobs
//! <data>/firmware/<pack>/modules/<name>.ko  prebuilt modules, vermagic-checked
//! <data>/firmware/<pack>/disabled           present while the pack is turned off
//! /lib/firmware/<path>                      symlink into the pack (made at boot and on install; the RAM root is writable)
//! /etc/modprobe.d/iyakki-<pack>.conf        the options, written at boot and on install
//! ```
//!
//! Why a symlink farm and not `firmware_class.path`: the kernel takes only one custom path and a USB-installed system
//! may want several packs at once; `/lib/firmware` is always searched and the root filesystem lives in RAM, so links
//! vanish with the session and are rebuilt by [`apply_boot`] from the data volume every boot.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::b64;
use crate::modinfo;
use crate::pack::{self, glob, Pack};
use crate::run::Runner;

#[derive(Clone, Debug)]
pub struct Fs {
    pub data: PathBuf,
    /// Where the kernel looks for firmware.
    pub fw_root: PathBuf,
    pub modprobe_d: PathBuf,
    pub proc: PathBuf,
    pub lib_modules: PathBuf,
}

impl Fs {
    pub fn real() -> Fs {
        Fs { data: "/var/lib/iyakki".into(), fw_root: "/lib/firmware".into(), modprobe_d: "/etc/modprobe.d".into(), proc: "/proc".into(), lib_modules: "/lib/modules".into() }
    }

    /// The file system for a data folder. On the device image (it has `/etc/iyakki`) that is the real `/lib/firmware`,
    /// `/etc/modprobe.d` and `/proc`; anywhere else (a desktop window, tests) the links and options go to a sandbox folder
    /// inside the data folder and nothing outside it is touched. The bool says whether it is the real thing.
    pub fn for_data(data: &Path) -> (Fs, bool) {
        if Path::new("/etc/iyakki").is_dir() {
            (Fs { data: data.to_path_buf(), ..Fs::real() }, true)
        } else {
            let root = data.join("sysroot");
            (Fs { data: data.to_path_buf(), fw_root: root.join("lib/firmware"), modprobe_d: root.join("etc/modprobe.d"), proc: PathBuf::from("/proc"), lib_modules: root.join("lib/modules") }, false)
        }
    }

    pub fn under(root: &Path) -> Fs {
        Fs { data: root.join("data"), fw_root: root.join("lib/firmware"), modprobe_d: root.join("etc/modprobe.d"), proc: root.join("proc"), lib_modules: root.join("lib/modules") }
    }

    pub fn pack_dir(&self, id: &str) -> PathBuf {
        self.data.join("firmware").join(id)
    }

    fn release(&self) -> String {
        std::fs::read_to_string(self.proc.join("sys/kernel/osrelease")).map(|s| s.trim().to_string()).unwrap_or_default()
    }

    /// The full vermagic of one stock module of the running kernel, when the image has any.
    pub fn reference_vermagic(&self) -> Option<String> {
        fn first_ko(dir: &Path, depth: u32) -> Option<PathBuf> {
            let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in &entries {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "ko") {
                    return Some(p);
                }
            }
            if depth > 0 {
                for e in entries {
                    if e.file_type().is_ok_and(|t| t.is_dir()) {
                        if let Some(f) = first_ko(&e.path(), depth - 1) {
                            return Some(f);
                        }
                    }
                }
            }
            None
        }
        let dir = self.lib_modules.join(self.release());
        let ko = first_ko(&dir, 6)?;
        modinfo::parse(&std::fs::read(ko).ok()?).ok().map(|m| m.vermagic)
    }

    fn loaded_modules(&self) -> Vec<String> {
        std::fs::read_to_string(self.proc.join("modules")).unwrap_or_default().lines().filter_map(|l| l.split_whitespace().next()).map(str::to_string).collect()
    }
}

/// What was recorded about an installed pack (the pack without its blobs, plus what we checked).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Record {
    pub id: String,
    pub version: String,
    /// Trust level id from the store (`official`, `verified`, `trusted`, `community`, `unsigned`, `local`).
    pub level: String,
    pub files: Vec<String>,
    pub modules: Vec<String>,
    pub modprobe: Vec<String>,
    pub load: Vec<String>,
    pub reload: Vec<String>,
    /// Drivers that would take the same hardware: unloaded (if loaded) before this pack's modules go in.
    pub unload: Vec<String>,
    pub cmdline: Vec<String>,
    pub matches: Vec<pack::Match>,
    pub why: String,
    pub license: String,
    pub source: String,
}

/// Trust levels allowed to bring kernel modules (code that runs inside the kernel).
pub fn level_may_bring_modules(level: &str) -> bool {
    matches!(level, "official" | "verified" | "trusted")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("tmp-iyakki");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<String>,
    /// Kernel command line suggestions (shown, never applied).
    pub cmdline: Vec<String>,
}

/// Check and write a pack to the data volume, then make it active. `level` is the store's trust level id.
pub fn install_pack(fs: &Fs, run: &dyn Runner, id: &str, version: &str, license: &str, source: &str, level: &str, p: &Pack) -> Result<Report, String> {
    if !pack::valid_module_name(id) {
        return Err("the pack name is not valid".into());
    }
    p.validate()?;
    let release = fs.release();
    // modules first: everything that can refuse does so before anything is written
    let mut kos = Vec::new();
    if p.has_modules() {
        if !level_may_bring_modules(level) {
            return Err(format!("this pack carries kernel modules, which run inside the kernel. They are accepted only from Official, Verified or Trusted publishers; this pack is {level}. Nothing was installed."));
        }
        let reference = fs.reference_vermagic();
        for m in &p.modules {
            let bytes = b64::decode(&m.data).ok_or("a module is not valid base64")?;
            if m.kernel != release {
                return Err(format!("module {} is for kernel {} but this system runs {}. Nothing was installed.", m.name, m.kernel, if release.is_empty() { "an unknown release" } else { &release }));
            }
            let info = modinfo::check(&bytes, &release, reference.as_deref()).map_err(|e| format!("module {}: {e} Nothing was installed.", m.name))?;
            kos.push((m.name.clone(), bytes, info));
        }
    }
    let dir = fs.pack_dir(id);
    let _ = std::fs::remove_dir_all(&dir);
    let mut rec = Record { id: id.into(), version: version.into(), level: level.into(), modprobe: p.modprobe.clone(), load: p.load.clone(), reload: p.reload.clone(), unload: p.unload.clone(), cmdline: p.cmdline.clone(), matches: p.matches.clone(), why: p.why.clone(), license: license.into(), source: source.into(), ..Default::default() };
    for f in &p.files {
        let bytes = b64::decode(&f.data).ok_or("a firmware file is not valid base64")?;
        write_atomic(&dir.join("files").join(&f.path), &bytes).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&dir);
        })?;
        rec.files.push(f.path.clone());
    }
    for (name, bytes, _) in &kos {
        write_atomic(&dir.join("modules").join(format!("{name}.ko")), bytes)?;
        rec.modules.push(name.clone());
    }
    write_atomic(&dir.join("pack.json"), serde_json::to_string_pretty(&rec).unwrap_or_default().as_bytes())?;
    let mut report = activate(fs, run, id)?;
    report.cmdline = p.cmdline.clone();
    if kos.iter().any(|(_, _, i)| !i.gpl_compatible()) {
        report.lines.push("a module here is not GPL-licensed: loading it marks the kernel as tainted".into());
    }
    Ok(report)
}

pub fn read_record(fs: &Fs, id: &str) -> Option<Record> {
    serde_json::from_str(&std::fs::read_to_string(fs.pack_dir(id).join("pack.json")).ok()?).ok()
}

pub fn installed(fs: &Fs) -> Vec<Record> {
    let mut v: Vec<Record> = std::fs::read_dir(fs.data.join("firmware")).into_iter().flatten().flatten().filter_map(|e| read_record(fs, &e.file_name().to_string_lossy())).collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

pub fn is_disabled(fs: &Fs, id: &str) -> bool {
    fs.pack_dir(id).join("disabled").exists()
}

pub fn set_disabled(fs: &Fs, run: &dyn Runner, id: &str, off: bool) -> Result<Report, String> {
    if read_record(fs, id).is_none() {
        return Err("that driver pack is not installed".into());
    }
    let marker = fs.pack_dir(id).join("disabled");
    if off {
        write_atomic(&marker, b"")?;
        unlink_farm(fs, id);
        let _ = std::fs::remove_file(fs.modprobe_d.join(format!("iyakki-{id}.conf")));
        Ok(Report { lines: vec!["turned off; takes full effect after a restart".into()], cmdline: vec![] })
    } else {
        let _ = std::fs::remove_file(marker);
        activate(fs, run, id)
    }
}

fn farm_links(fs: &Fs, id: &str) -> Vec<(PathBuf, PathBuf)> {
    let Some(rec) = read_record(fs, id) else { return vec![] };
    rec.files.iter().map(|f| (fs.fw_root.join(f), fs.pack_dir(id).join("files").join(f))).collect()
}

fn unlink_farm(fs: &Fs, id: &str) {
    for (link, target) in farm_links(fs, id) {
        if std::fs::read_link(&link).is_ok_and(|t| t == target) {
            let _ = std::fs::remove_file(&link);
        }
    }
}

/// Make an installed pack active now: firmware links, modprobe options, modules, and the modules it wants loaded or reloaded.
pub fn activate(fs: &Fs, run: &dyn Runner, id: &str) -> Result<Report, String> {
    let rec = read_record(fs, id).ok_or("that driver pack is not installed")?;
    let mut rep = Report::default();
    for (link, target) in farm_links(fs, id) {
        if let Some(d) = link.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        match std::fs::symlink_metadata(&link) {
            Ok(m) if m.file_type().is_symlink() => {
                let _ = std::fs::remove_file(&link);
            }
            Ok(_) => {
                rep.lines.push(format!("{} already exists in the image: its own copy is kept", link.strip_prefix(&fs.fw_root).unwrap_or(&link).display()));
                continue;
            }
            Err(_) => {}
        }
        std::os::unix::fs::symlink(&target, &link).map_err(|e| format!("{}: {e}", link.display()))?;
    }
    let conf = fs.modprobe_d.join(format!("iyakki-{id}.conf"));
    if rec.modprobe.is_empty() {
        let _ = std::fs::remove_file(&conf);
    } else {
        write_atomic(&conf, format!("# written by the IYAKKI driver pack {id}; edit the pack, not this file\n{}\n", rec.modprobe.join("\n")).as_bytes())?;
    }
    // modules that were installed: check again (the kernel may have been replaced since) and insert
    let release = fs.release();
    let reference = fs.reference_vermagic();
    for m in &rec.unload {
        if !rec.modules.is_empty() && fs.loaded_modules().iter().any(|l| l == m) {
            match run.run("modprobe", &["-r", m]) {
                Ok(()) => rep.lines.push(format!("driver {m} unloaded: it would take the same hardware")),
                Err(e) => rep.lines.push(format!("driver {m} could not be unloaded ({e}); restart to be sure it does not take the hardware")),
            }
        }
    }
    for name in &rec.modules {
        let path = fs.pack_dir(id).join("modules").join(format!("{name}.ko"));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        match modinfo::check(&bytes, &release, reference.as_deref()) {
            Ok(_) if !level_may_bring_modules(&rec.level) => rep.lines.push(format!("module {name} skipped: the pack is {} now", rec.level)),
            Ok(_) if fs.loaded_modules().iter().any(|l| l == name) => rep.lines.push(format!("module {name} is loaded already")),
            Ok(info) => {
                // insmod does not look for dependencies: load them first (the stock kernel modules the module was linked against)
                for dep in &info.depends {
                    if pack::valid_module_name(dep) && !fs.loaded_modules().iter().any(|l| l == dep) {
                        let _ = run.run("modprobe", &[dep]);
                    }
                }
                match run.run("insmod", &[&path.to_string_lossy()]) {
                    Ok(()) => rep.lines.push(format!("module {name} inserted")),
                    Err(e) => rep.lines.push(format!("module {name} was not inserted: {e}")),
                }
            }
            Err(e) => rep.lines.push(format!("module {name} skipped: {e}")),
        }
    }
    let loaded = fs.loaded_modules();
    for m in &rec.reload {
        if loaded.iter().any(|l| l == m) {
            let _ = run.run("modprobe", &["-r", m]);
        }
        match run.run("modprobe", &[m]) {
            Ok(()) => rep.lines.push(format!("driver {m} reloaded")),
            Err(e) => rep.lines.push(format!("driver {m} did not load: {e}")),
        }
    }
    for m in rec.load.iter().filter(|m| !rec.reload.contains(m) && !rec.modules.contains(m)) {
        match run.run("modprobe", &[m]) {
            Ok(()) => rep.lines.push(format!("driver {m} loaded")),
            Err(e) => rep.lines.push(format!("driver {m} did not load: {e}")),
        }
    }
    rep.cmdline = rec.cmdline.clone();
    Ok(rep)
}

/// At every boot, after the data volume is mounted and before the network, sound and Bluetooth drivers load.
/// Returns log lines. `reload` of already-loaded drivers is skipped here (they have not loaded yet in late boot).
pub fn apply_boot(fs: &Fs, run: &dyn Runner) -> Vec<String> {
    let mut log = Vec::new();
    for rec in installed(fs) {
        if is_disabled(fs, &rec.id) {
            log.push(format!("driver pack {}: turned off", rec.id));
            continue;
        }
        match activate(fs, run, &rec.id) {
            Ok(r) => log.push(format!("driver pack {}: {} file(s) linked{}", rec.id, rec.files.len(), if r.lines.is_empty() { String::new() } else { format!("; {}", r.lines.join("; ")) })),
            Err(e) => log.push(format!("driver pack {}: {e}", rec.id)),
        }
    }
    log
}

/// Remove a pack: links, options, files. Loaded modules stay loaded until the next restart (said so).
pub fn remove_pack(fs: &Fs, id: &str) -> Result<Report, String> {
    let rec = read_record(fs, id).ok_or("that driver pack is not installed")?;
    unlink_farm(fs, id);
    let _ = std::fs::remove_file(fs.modprobe_d.join(format!("iyakki-{id}.conf")));
    std::fs::remove_dir_all(fs.pack_dir(id)).map_err(|e| format!("{}: {e}", fs.pack_dir(id).display()))?;
    let mut lines = vec![];
    if rec.modules.iter().chain(&rec.load).any(|m| fs.loaded_modules().contains(m)) {
        lines.push("a driver from this pack is still loaded: restart to unload it".into());
    }
    Ok(Report { lines, cmdline: vec![] })
}

/// Is a firmware file with this path prefix reachable (in the kernel's folder or in an installed, active pack)?
pub fn firmware_present(fs: &Fs, prefix: &str) -> bool {
    fn has(dir: &Path, rel_prefix: &str) -> bool {
        let (sub, stem) = match rel_prefix.rsplit_once('/') {
            Some((d, s)) => (dir.join(d), s),
            None => (dir.to_path_buf(), rel_prefix),
        };
        std::fs::read_dir(sub).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with(stem))
    }
    has(&fs.fw_root, prefix) || installed(fs).iter().filter(|r| !is_disabled(fs, &r.id)).any(|r| has(&fs.pack_dir(&r.id).join("files"), prefix))
}

/// Ids of installed packs that are turned on.
pub fn active_ids(fs: &Fs) -> std::collections::BTreeSet<String> {
    installed(fs).into_iter().filter(|r| !is_disabled(fs, &r.id)).map(|r| r.id).collect()
}

/// Build a local pack from files the person supplied in a folder (a USB stick), accepting only the names the entry
/// lists. Returns the pack (unsigned, trust `local`) and the list of what was taken, with checksums.
pub fn pack_from_folder(known: &crate::known::Known, dir: &Path) -> Result<(Pack, Vec<String>), String> {
    if known.accept.is_empty() {
        return Err(format!("{} does not take files from a folder", known.title));
    }
    let mut found: Vec<PathBuf> = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((d, depth)) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let p = e.path();
            let Ok(t) = e.file_type() else { continue };
            if t.is_symlink() {
                continue;
            }
            if t.is_dir() && depth < 2 {
                stack.push((p, depth + 1));
            } else if t.is_file() {
                found.push(p);
            }
        }
    }
    found.sort();
    let mut pack = Pack { v: 1, matches: known.match_rules(), why: known.why.into(), ..Default::default() };
    let mut taken = Vec::new();
    for p in found {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some((_, sub)) = known.accept.iter().find(|(pat, _)| glob(pat, &name)) else { continue };
        let rel = format!("{sub}/{name}");
        if !pack::safe_rel_path(&rel) || pack.files.iter().any(|f| f.path == rel) {
            continue;
        }
        let meta = std::fs::metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if meta.len() == 0 || meta.len() as usize > pack::MAX_FILE_BYTES {
            continue;
        }
        let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        taken.push(format!("{rel} ({} bytes, sha256 {})", bytes.len(), &pack::sha256_hex(&bytes)[..16]));
        pack.files.push(pack::FwFile { path: rel, data: b64::encode(&bytes), sha256: pack::sha256_hex(&bytes) });
    }
    if pack.files.is_empty() {
        return Err(format!("no file that {} accepts was found in {} (looking for {})", known.title, dir.display(), known.accept.iter().map(|a| a.0).collect::<Vec<_>>().join(", ")));
    }
    pack.reload = known.drivers.iter().map(|d| (*d).to_string()).take(1).collect();
    pack.validate()?;
    Ok((pack, taken))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::Recorder;

    fn fs(tag: &str) -> (Fs, PathBuf) {
        let root = std::env::temp_dir().join(format!("iyakki-drivers-apply-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("proc/sys/kernel")).unwrap();
        std::fs::write(root.join("proc/sys/kernel/osrelease"), "6.12.112\n").unwrap();
        std::fs::write(root.join("proc/modules"), "btusb 1 0 - Live 0x0\n").unwrap();
        (Fs::under(&root), root)
    }

    fn fwpack() -> Pack {
        Pack {
            v: 1,
            files: vec![pack::FwFile { path: "brcm/BCM20702A1-05ac-8289.hcd".into(), data: b64::encode(b"HCD-BYTES"), sha256: String::new() }],
            modprobe: vec!["options hid_apple fnmode=2".into()],
            reload: vec!["btusb".into()],
            cmdline: vec!["acpi_osi=Darwin".into()],
            ..Default::default()
        }
    }

    #[test]
    fn a_firmware_pack_lands_on_the_data_volume_is_linked_where_the_kernel_looks_and_reloads_the_driver() {
        let (f, _) = fs("fw");
        let run = Recorder::default();
        let rep = install_pack(&f, &run, "bt-fw", "1.0.0", "Proprietary", "usb stick", "local", &fwpack()).unwrap();
        let blob = f.data.join("firmware/bt-fw/files/brcm/BCM20702A1-05ac-8289.hcd");
        assert_eq!(std::fs::read(&blob).unwrap(), b"HCD-BYTES");
        let link = f.fw_root.join("brcm/BCM20702A1-05ac-8289.hcd");
        assert_eq!(std::fs::read_link(&link).unwrap(), blob);
        assert_eq!(std::fs::read(&link).unwrap(), b"HCD-BYTES", "readable through the link");
        let conf = std::fs::read_to_string(f.modprobe_d.join("iyakki-bt-fw.conf")).unwrap();
        assert!(conf.contains("options hid_apple fnmode=2"));
        let calls = run.calls.lock().unwrap().clone();
        assert_eq!(calls, vec!["modprobe -r btusb", "modprobe btusb"], "a loaded driver is unloaded and loaded again so it finds the file");
        assert_eq!(rep.cmdline, vec!["acpi_osi=Darwin"], "kernel options are suggested, not applied");
        assert!(firmware_present(&f, "brcm/BCM20702A1"));
        assert_eq!(installed(&f).len(), 1);
        assert_eq!(read_record(&f, "bt-fw").unwrap().level, "local");
    }

    #[test]
    fn boot_rebuilds_the_links_in_a_fresh_ram_root_and_keeps_the_images_own_files() {
        let (f, _) = fs("boot");
        let run = Recorder::default();
        install_pack(&f, &run, "bt-fw", "1.0.0", "x", "usb", "local", &fwpack()).unwrap();
        // the next boot: RAM root is empty again, the data volume still has the pack
        std::fs::remove_dir_all(&f.fw_root).unwrap();
        std::fs::remove_dir_all(&f.modprobe_d).unwrap();
        std::fs::create_dir_all(f.fw_root.join("brcm")).unwrap();
        std::fs::write(f.fw_root.join("brcm/other.bin"), b"image").unwrap();
        let log = apply_boot(&f, &run);
        assert!(log.iter().any(|l| l.contains("bt-fw") && l.contains("1 file")), "{log:?}");
        assert!(f.fw_root.join("brcm/BCM20702A1-05ac-8289.hcd").exists() && f.modprobe_d.join("iyakki-bt-fw.conf").exists());
        // a file the image already has is not replaced
        let mut p = fwpack();
        p.files[0].path = "brcm/other.bin".into();
        let r = install_pack(&f, &run, "clash", "1.0.0", "x", "usb", "local", &p).unwrap();
        assert!(r.lines.iter().any(|l| l.contains("already exists in the image")), "{:?}", r.lines);
        assert_eq!(std::fs::read(f.fw_root.join("brcm/other.bin")).unwrap(), b"image");
    }

    #[test]
    fn remove_and_turning_off_take_the_links_and_options_away() {
        let (f, _) = fs("rm");
        let run = Recorder::default();
        install_pack(&f, &run, "bt-fw", "1.0.0", "x", "usb", "local", &fwpack()).unwrap();
        set_disabled(&f, &run, "bt-fw", true).unwrap();
        assert!(is_disabled(&f, "bt-fw") && !f.fw_root.join("brcm/BCM20702A1-05ac-8289.hcd").exists() && !f.modprobe_d.join("iyakki-bt-fw.conf").exists());
        assert!(apply_boot(&f, &run).iter().any(|l| l.contains("turned off")));
        assert!(!f.fw_root.join("brcm/BCM20702A1-05ac-8289.hcd").exists(), "a turned-off pack is not linked at boot");
        set_disabled(&f, &run, "bt-fw", false).unwrap();
        assert!(f.fw_root.join("brcm/BCM20702A1-05ac-8289.hcd").exists());
        let r = remove_pack(&f, "bt-fw").unwrap();
        assert!(installed(&f).is_empty() && !f.fw_root.join("brcm/BCM20702A1-05ac-8289.hcd").exists() && !f.data.join("firmware/bt-fw").exists());
        assert!(r.lines.is_empty());
        assert!(remove_pack(&f, "bt-fw").is_err());
    }

    fn modpack(vermagic: &str, kernel: &str) -> Pack {
        Pack { v: 1, modules: vec![pack::KoFile { name: "wl".into(), kernel: kernel.into(), data: b64::encode(&modinfo::synthetic_module("wl", vermagic, "MIXED/Proprietary")), sha256: String::new() }], load: vec!["wl".into()], ..Default::default() }
    }

    #[test]
    fn a_module_for_another_kernel_is_refused_and_nothing_is_written() {
        let (f, _) = fs("vm");
        let run = Recorder::default();
        let e = install_pack(&f, &run, "wl-pack", "1.0.0", "x", "usb", "official", &modpack("6.12.100 SMP mod_unload ", "6.12.100")).unwrap_err();
        assert!(e.contains("6.12.100") && e.contains("6.12.112") && e.contains("Nothing was installed"), "{e}");
        assert!(!f.pack_dir("wl-pack").exists() && run.calls.lock().unwrap().is_empty());
        // the pack says one kernel and the module another: also refused
        let e = install_pack(&f, &run, "wl-pack", "1.0.0", "x", "usb", "official", &modpack("6.12.100 SMP mod_unload ", "6.12.112")).unwrap_err();
        assert!(e.contains("6.12.100"), "{e}");
    }

    #[test]
    fn modules_need_a_good_signature_chain_and_a_matching_kernel() {
        let (f, _) = fs("trust");
        let run = Recorder::default();
        for level in ["community", "unsigned", "local"] {
            let e = install_pack(&f, &run, "wl-pack", "1.0.0", "x", "usb", level, &modpack("6.12.112 SMP mod_unload ", "6.12.112")).unwrap_err();
            assert!(e.contains("Official, Verified or Trusted") && e.contains(level), "{e}");
            assert!(!f.pack_dir("wl-pack").exists());
        }
        let rep = install_pack(&f, &run, "wl-pack", "1.0.0", "x", "usb", "verified", &modpack("6.12.112 SMP mod_unload ", "6.12.112")).unwrap();
        let calls = run.calls.lock().unwrap().clone();
        assert!(calls[0].starts_with("insmod ") && calls[0].ends_with("wl.ko") && !calls.contains(&"modprobe wl".to_string()), "a module the pack carries is inserted by path, not looked up: {calls:?}");
        assert!(rep.lines.iter().any(|l| l.contains("tainted")));
        // a stock module of the running kernel with different build flags is caught too
        let d = f.lib_modules.join("6.12.112/kernel");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("stock.ko"), modinfo::synthetic_module("stock", "6.12.112 SMP preempt mod_unload ", "GPL")).unwrap();
        let e = install_pack(&f, &run, "wl-pack2", "1.0.0", "x", "usb", "verified", &modpack("6.12.112 SMP mod_unload ", "6.12.112")).unwrap_err();
        assert!(e.contains("build settings"), "{e}");
    }

    #[test]
    fn a_module_gets_its_dependencies_first_and_the_drivers_that_would_take_its_hardware_are_unloaded() {
        let (f, _) = fs("deps");
        std::fs::write(f.proc.join("modules"), "b43 1 0 - Live 0x0\nbcma 1 1 b43, Live 0x0\n").unwrap();
        let run = Recorder::default();
        let ko = modinfo::synthetic_module_with("wl", "6.12.112 SMP mod_unload ", "MIXED/Proprietary", "cfg80211");
        let p = Pack { v: 1, modules: vec![pack::KoFile { name: "wl".into(), kernel: "6.12.112".into(), data: b64::encode(&ko), sha256: pack::sha256_hex(&ko) }], unload: vec!["b43".into(), "bcma".into()], modprobe: vec!["blacklist b43".into(), "blacklist bcma".into()], load: vec!["wl".into()], ..Default::default() };
        let rep = install_pack(&f, &run, "wl-pack", "1.0.0", "x", "usb", "trusted", &p).unwrap();
        let calls = run.calls.lock().unwrap().clone();
        let at = |c: &str| calls.iter().position(|x| x.starts_with(c)).unwrap_or_else(|| panic!("{c} in {calls:?}"));
        assert!(at("modprobe -r b43") < at("modprobe -r bcma") && at("modprobe -r bcma") < at("modprobe cfg80211") && at("modprobe cfg80211") < at("insmod "), "{calls:?}");
        assert!(rep.lines.iter().any(|l| l.contains("module wl inserted")) && rep.lines.iter().any(|l| l.contains("b43 unloaded")));
        assert!(std::fs::read_to_string(f.modprobe_d.join("iyakki-wl-pack.conf")).unwrap().contains("blacklist bcma"));
        // already loaded: not inserted twice
        std::fs::write(f.proc.join("modules"), "wl 1 0 - Live 0x0\ncfg80211 1 1 wl, Live 0x0\n").unwrap();
        let run2 = Recorder::default();
        let rep = activate(&f, &run2, "wl-pack").unwrap();
        assert!(rep.lines.iter().any(|l| l.contains("loaded already")) && !run2.calls.lock().unwrap().iter().any(|c| c.starts_with("insmod")), "{:?}", rep.lines);
    }

    #[test]
    fn a_module_is_checked_again_at_boot_and_skipped_if_the_kernel_changed() {
        let (f, _) = fs("reboot");
        let run = Recorder::default();
        install_pack(&f, &run, "wl-pack", "1.0.0", "x", "usb", "trusted", &modpack("6.12.112 SMP mod_unload ", "6.12.112")).unwrap();
        std::fs::write(f.proc.join("sys/kernel/osrelease"), "6.12.113\n").unwrap();
        let run2 = Recorder::default();
        let log = apply_boot(&f, &run2);
        assert!(log.iter().any(|l| l.contains("skipped") && l.contains("6.12.113")), "{log:?}");
        assert!(!run2.calls.lock().unwrap().iter().any(|c| c.starts_with("insmod")));
    }

    #[test]
    fn a_pack_from_a_usb_folder_takes_only_the_names_the_entry_accepts() {
        let (f, root) = fs("folder");
        let stick = root.join("stick");
        std::fs::create_dir_all(stick.join("sub")).unwrap();
        std::fs::write(stick.join("BCM20702A1-05ac-8289.hcd"), b"HCD").unwrap();
        std::fs::write(stick.join("sub/BCM20702A0-0a5c-21e6.hcd"), b"HCD2").unwrap();
        std::fs::write(stick.join("notes.txt"), b"not firmware").unwrap();
        std::fs::write(stick.join("evil.sh"), b"#!/bin/sh").unwrap();
        let known = crate::known::by_id("mac-bt-broadcom").unwrap();
        let (p, taken) = pack_from_folder(known, &stick).unwrap();
        let mut paths: Vec<&str> = p.files.iter().map(|x| x.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["brcm/BCM20702A0-0a5c-21e6.hcd", "brcm/BCM20702A1-05ac-8289.hcd"]);
        assert_eq!(taken.len(), 2);
        assert!(!p.matches.is_empty());
        install_pack(&f, &Recorder::default(), known.id, "1.0.0", known.license, "usb stick", "local", &p).unwrap();
        assert!(firmware_present(&f, "brcm/BCM20702A1"));
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(pack_from_folder(known, &empty).unwrap_err().contains("no file"));
        assert!(pack_from_folder(crate::known::by_id("mac-t2").unwrap(), &stick).is_err());
    }
}

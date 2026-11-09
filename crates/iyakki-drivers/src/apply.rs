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
    // a T2 Mac keeps its Wi-Fi and Bluetooth drivers' files aside until the firmware is here (t2hold.rs): give back what this pack loads
    let wanted: Vec<String> = rec.reload.iter().chain(&rec.load).cloned().collect();
    let back = crate::t2hold::release(&fs.lib_modules, &wanted);
    if back > 0 {
        rep.lines.push(format!("{back} driver file(s) given back (they were held until the firmware existed)"));
    }
    // a Bluetooth pack (whatever files it has) gives back the whole Bluetooth group: hci_uart binds the BCM4364 UART controller of the
    // Mac mini 2018 even when the pack only has the PCIe chip's files
    if rec.id == BT_PACK && !is_disabled(fs, &rec.id) {
        rep.lines.extend(give_back_bluetooth(fs, run, &wanted, "a Bluetooth driver pack is installed"));
    }
    let loaded = fs.loaded_modules();
    // a core module in a pack's lists (packs recorded before the rule): never reloaded, see pack::CORE_MODULES
    let core = |m: &String| pack::CORE_MODULES.contains(&m.as_str());
    for m in rec.reload.iter().filter(|m| core(m)).chain(rec.load.iter().filter(|m| core(m) && !rec.reload.contains(m))) {
        rep.lines.push(format!("driver {m} not reloaded: a core module (its transport drivers are enough)"));
    }
    for m in rec.reload.iter().filter(|m| !core(m)) {
        if loaded.iter().any(|l| l == m) {
            let _ = run.run("modprobe", &["-r", m]);
        }
        match run.run("modprobe", &[m]) {
            Ok(()) => rep.lines.push(format!("driver {m} reloaded")),
            Err(e) => rep.lines.push(format!("driver {m} did not load: {e}")),
        }
    }
    for m in rec.load.iter().filter(|m| !core(m) && !rec.reload.contains(m) && !rec.modules.contains(m)) {
        match run.run("modprobe", &[m]) {
            Ok(()) => rep.lines.push(format!("driver {m} loaded")),
            Err(e) => rep.lines.push(format!("driver {m} did not load: {e}")),
        }
    }
    rep.cmdline = rec.cmdline.clone();
    Ok(rep)
}

/// The id of the Bluetooth driver pack (`macfw::PACK_BT`).
pub const BT_PACK: &str = "mac-bt-broadcom";

/// Give every held Bluetooth driver file back (`btbcm`, `hci_uart`, `hci_bcm4377`; nothing happens when none is held: a machine that
/// is not a T2 Mac, or one whose firmware exists) and load them in dependency order. The modules in `skip` are only given back: the
/// caller loads them itself. Returns log lines.
pub fn give_back_bluetooth(fs: &Fs, run: &dyn Runner, skip: &[String], why: &str) -> Vec<String> {
    let group: Vec<String> = crate::t2hold::BT_GROUP.iter().map(|s| s.to_string()).collect();
    let back = crate::t2hold::release_names(&fs.lib_modules, &group);
    if back.is_empty() {
        return vec![];
    }
    let mut lines = vec![format!("Bluetooth: {} driver file(s) given back ({why}): {}", back.len(), back.join(", "))];
    let loaded = fs.loaded_modules();
    // btbcm first (the Broadcom part the others use), then the UART and PCIe drivers
    for m in crate::t2hold::BT_GROUP {
        let wanted = back.iter().any(|b| b == m) || (*m == "btbcm");
        if !wanted || loaded.iter().any(|l| l == m) || skip.iter().any(|k| k.replace('-', "_") == *m) {
            continue;
        }
        match run.run("modprobe", &[m]) {
            Ok(()) => lines.push(format!("driver {m} loaded")),
            Err(e) => lines.push(format!("driver {m} did not load: {e}")),
        }
    }
    lines
}

/// The person chose "Load the Bluetooth driver anyway": give the held Bluetooth drivers back and load them even though no firmware
/// file exists (the controller then runs in its ROM state). The choice is remembered on the data volume, so it is applied again at
/// every start ([`apply_boot`]). Nothing is written when nothing is held (not a T2 Mac, or the firmware exists).
pub fn load_bluetooth_anyway(fs: &Fs, run: &dyn Runner) -> Vec<String> {
    let lines = give_back_bluetooth(fs, run, &[], "chosen: load anyway");
    if lines.is_empty() {
        return vec!["Nothing was held back: the Bluetooth drivers are not set aside on this machine.".into()];
    }
    let mark = fs.data.join(crate::t2hold::LOAD_ANYWAY_MARK);
    if let Some(d) = mark.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&mark, b"the person chose to load the Bluetooth drivers without firmware\n");
    lines
}

/// At every boot, after the data volume is mounted and before the network, sound and Bluetooth drivers load.
/// Returns log lines. `reload` of already-loaded drivers is skipped here (they have not loaded yet in late boot).
pub fn apply_boot(fs: &Fs, run: &dyn Runner) -> Vec<String> {
    apply_boot_for(fs, run, &crate::packfit::Machine::default())
}

/// [`apply_boot`] on a known machine: a pack that does not fit it ([`crate::packfit::skip_reason`]) is left aside, its files not
/// linked and its drivers not loaded again, with the reason in the log. The pack stays installed for the machines it fits.
pub fn apply_boot_for(fs: &Fs, run: &dyn Runner, machine: &crate::packfit::Machine) -> Vec<String> {
    let mut log = Vec::new();
    for rec in installed(fs) {
        if is_disabled(fs, &rec.id) {
            log.push(format!("driver pack {}: turned off", rec.id));
            continue;
        }
        if let Some(why) = crate::packfit::skip_reason(&rec.id, machine) {
            unlink_farm(fs, &rec.id);
            log.push(format!("driver pack {}: {why}", rec.id));
            continue;
        }
        match activate(fs, run, &rec.id) {
            Ok(r) => log.push(format!("driver pack {}: {} file(s) linked{}", rec.id, rec.files.len(), if r.lines.is_empty() { String::new() } else { format!("; {}", r.lines.join("; ")) })),
            Err(e) => log.push(format!("driver pack {}: {e}", rec.id)),
        }
    }
    // the person chose "Load the Bluetooth driver anyway" earlier: the RAM root set the files aside again at this boot
    if fs.data.join(crate::t2hold::LOAD_ANYWAY_MARK).exists() {
        log.extend(give_back_bluetooth(fs, run, &[], "chosen earlier: load anyway"));
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
    let mut named = Vec::new();
    for p in found {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !known.accept.iter().any(|(pat, _)| glob(pat, &name)) {
            continue;
        }
        let meta = std::fs::metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if meta.len() == 0 || meta.len() as usize > pack::MAX_FILE_BYTES {
            continue;
        }
        named.push((name, std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?));
    }
    pack_from_named(known, &named, &dir.display().to_string())
}

/// Build a local pack from files already in memory (`(file name, bytes)`: a folder's files, or what `appfw` extracted), accepting
/// only the names the entry lists. `from` says where they came from, for the error text.
pub fn pack_from_named(known: &crate::known::Known, files: &[(String, Vec<u8>)], from: &str) -> Result<(Pack, Vec<String>), String> {
    if known.accept.is_empty() {
        return Err(format!("{} does not take files from a folder", known.title));
    }
    let mut pack = Pack { v: 1, matches: known.match_rules(), why: known.why.into(), ..Default::default() };
    let mut taken = Vec::new();
    for (name, bytes) in files {
        let Some((_, sub)) = known.accept.iter().find(|(pat, _)| glob(pat, name)) else { continue };
        let rel = format!("{sub}/{name}");
        if !pack::safe_rel_path(&rel) || pack.files.iter().any(|f| f.path == rel) || bytes.is_empty() || bytes.len() > pack::MAX_FILE_BYTES {
            continue;
        }
        taken.push(format!("{rel} ({} bytes, sha256 {})", bytes.len(), &pack::sha256_hex(bytes)[..16]));
        pack.files.push(pack::FwFile { path: rel, data: b64::encode(bytes), sha256: pack::sha256_hex(bytes) });
    }
    if pack.files.is_empty() {
        return Err(format!("no file that {} accepts was found in {from} (looking for {})", known.title, known.accept.iter().map(|a| a.0).collect::<Vec<_>>().join(", ")));
    }
    pack.reload = reload_for(known, &pack);
    pack.validate()?;
    Ok((pack, taken))
}

/// The drivers to load again once the firmware is in place. The first driver of the entry, except for the Bluetooth entry,
/// where it depends on the files: `brcm/BCM.hcd` is what the UART Bluetooth of the BCM4364 asks for (`hci_uart`), `brcmbt*`
/// what the PCIe one asks for (`hci_bcm4377`); a T2 Mac holds those two back until then (`t2hold.rs`).
pub fn reload_for(known: &crate::known::Known, pack: &Pack) -> Vec<String> {
    if known.id == "mac-bt-broadcom" {
        let mut v: Vec<String> = Vec::new();
        // a .hcd without the USB ids in its name (`BCM.hcd`, `BCM4350C0.hcd`) is a UART controller's; with `-vvvv-pppp` it is a USB one's
        let usb_shaped = |p: &str| {
            let b = p.trim_end_matches(".hcd").rsplit('-').take(2).collect::<Vec<_>>();
            b.len() == 2 && b.iter().all(|x| x.len() == 4 && x.chars().all(|c| c.is_ascii_hexdigit()))
        };
        if pack.files.iter().any(|f| f.path.ends_with(".hcd") && !usb_shaped(&f.path)) {
            v.push("hci_uart".into());
        }
        if pack.files.iter().any(|f| f.path.starts_with("brcm/brcmbt")) {
            v.push("hci_bcm4377".into());
        }
        if pack.files.iter().any(|f| f.path.ends_with(".hcd") && usb_shaped(&f.path)) {
            v.push("btusb".into());
        }
        return v;
    }
    known.drivers.iter().map(|d| (*d).to_string()).take(1).collect()
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

    /// The data volume travels between machines: at start-up the Mac Bluetooth pack is left aside on a non-T2 Mac whose chip sits on
    /// the serial link (MacBookPro14,1, 2026-11-08: the patch is refused and wedges the chip), and linked and loaded on a T2 Mac.
    #[test]
    fn boot_leaves_a_pack_that_does_not_fit_this_machine_aside_and_links_it_on_one_it_fits() {
        use crate::packfit::Machine;
        let (f, _) = fs("packfit");
        let run = Recorder::default();
        install_pack(&f, &run, "mac-bt-broadcom", "1.0.0", "x", "usb", "local", &fwpack()).unwrap();
        let link = f.fw_root.join("brcm/BCM20702A1-05ac-8289.hcd");
        assert!(link.exists(), "installed: linked");
        // the next boot on the 2017 MacBook Pro: nothing linked, no driver loaded, the reason in the log, the pack kept
        run.calls.lock().unwrap().clear();
        let mbp = Machine::from_facts("Apple Inc.", "MacBookPro14,1", true);
        let log = apply_boot_for(&f, &run, &mbp);
        assert!(log.iter().any(|l| l.starts_with("driver pack mac-bt-broadcom: left out on this machine (MacBookPro14,1)")), "{log:?}");
        assert!(!link.exists(), "the link from the earlier install is taken away again");
        assert!(run.calls.lock().unwrap().is_empty(), "no modprobe: {:?}", run.calls.lock().unwrap());
        assert_eq!(installed(&f).len(), 1, "the pack stays for the machines it fits");
        // the same volume in the T2 Mac mini: linked and loaded as before
        let mini = Machine::from_facts("Apple Inc.", "Macmini8,1", true);
        let log = apply_boot_for(&f, &run, &mini);
        assert!(log.iter().any(|l| l.contains("mac-bt-broadcom") && l.contains("1 file(s) linked")), "{log:?}");
        assert!(link.exists());
        assert!(run.calls.lock().unwrap().contains(&"modprobe btusb".to_string()), "{:?}", run.calls.lock().unwrap());
    }

    /// A pack recorded with the `bluetooth` core module in its reload list (the generic Hardware firmware pack of a Lenovo's MediaTek
    /// file, on a stick that then booted a MacBook Pro): the core is never reloaded, the transport driver still is.
    #[test]
    fn a_core_module_in_a_packs_reload_list_is_never_reloaded_but_the_transport_driver_is() {
        let (f, _) = fs("core-reload");
        let run = Recorder::default();
        let mut p = fwpack();
        p.reload = vec!["bluetooth".into(), "btusb".into()];
        let rep = install_pack(&f, &run, "fw-hardware", "1.0.0", "x", "usb", "local", &p).unwrap();
        let calls = run.calls.lock().unwrap().clone();
        assert!(!calls.iter().any(|c| c.contains("bluetooth")), "{calls:?}");
        assert!(calls.contains(&"modprobe btusb".to_string()), "{calls:?}");
        assert!(rep.lines.iter().any(|l| l.contains("driver bluetooth not reloaded: a core module")), "{:?}", rep.lines);
        run.calls.lock().unwrap().clear();
        let log = apply_boot(&f, &run);
        assert!(log.iter().any(|l| l.contains("fw-hardware") && l.contains("bluetooth not reloaded") && l.contains("btusb reloaded")), "{log:?}");
        assert!(!run.calls.lock().unwrap().iter().any(|c| c.contains("bluetooth")));
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

    // ---- the T2 hold-back and the Bluetooth group

    const MODS: [&str; 5] = ["kernel/drivers/net/wireless/broadcom/brcm80211/brcmfmac/brcmfmac.ko", "kernel/drivers/bluetooth/hci_uart.ko", "kernel/drivers/bluetooth/btbcm.ko", "kernel/drivers/bluetooth/hci_bcm4377.ko", "kernel/drivers/bluetooth/btusb.ko"];

    /// A fake machine: module files under lib/modules, held back the way boot does it for this DMI product.
    fn t2_fs(tag: &str, product: &str) -> (Fs, PathBuf) {
        let (f, root) = fs(tag);
        for m in MODS {
            let p = f.lib_modules.join("6.12.112").join(m);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"ko").unwrap();
        }
        hold(&f, &root, product);
        (f, root)
    }

    fn hold(f: &Fs, root: &Path, product: &str) {
        std::fs::write(root.join("product_name"), product).unwrap();
        crate::t2hold::apply(&root.join("product_name"), &f.fw_root, &f.modprobe_d, &f.lib_modules);
    }

    fn file(f: &Fs, name: &str) -> (bool, bool) {
        let dir = f.lib_modules.join("6.12.112/kernel/drivers");
        let p = ["bluetooth", "net/wireless/broadcom/brcm80211/brcmfmac"].iter().map(|d| dir.join(d).join(name)).find(|p| p.exists() || PathBuf::from(format!("{}{}", p.display(), crate::t2hold::HELD_SUFFIX)).exists()).unwrap();
        (p.exists(), PathBuf::from(format!("{}{}", p.display(), crate::t2hold::HELD_SUFFIX)).exists())
    }

    fn pack_of(path: &str, reload: &[&str]) -> Pack {
        Pack { v: 1, files: vec![pack::FwFile { path: path.into(), data: b64::encode(b"FW"), sha256: String::new() }], reload: reload.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn a_wifi_only_install_gives_back_wifi_and_keeps_the_bluetooth_drivers_held() {
        let (f, _) = t2_fs("held-wifi", "Macmini8,1");
        assert_eq!(file(&f, "hci_uart.ko"), (false, true));
        let run = Recorder::default();
        install_pack(&f, &run, "mac-wifi-brcmfmac", "1", "x", "usb", "local", &pack_of("brcm/brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin", &["brcmfmac"])).unwrap();
        assert_eq!(file(&f, "brcmfmac.ko"), (true, false), "the Wi-Fi driver is back");
        assert_eq!(file(&f, "hci_uart.ko"), (false, true), "no Bluetooth pack and nothing chosen: still held");
        assert_eq!(file(&f, "hci_bcm4377.ko"), (false, true));
        assert_eq!(run.calls.lock().unwrap().clone(), vec!["modprobe brcmfmac"]);
    }

    #[test]
    fn a_bluetooth_pack_gives_back_the_uart_driver_and_btbcm_even_when_its_files_are_for_the_pcie_chip() {
        let (f, _) = t2_fs("held-bt", "Macmini8,1");
        let run = Recorder::default();
        // the Mac mini case from the real machine: the pack has brcmbt files (hci_bcm4377 reloaded) but no BCM.hcd
        let rep = install_pack(&f, &run, "mac-bt-broadcom", "1", "x", "usb", "local", &pack_of("brcm/brcmbt4377b3-apple,formosa-m.bin", &["hci_bcm4377"])).unwrap();
        assert_eq!(file(&f, "hci_uart.ko"), (true, false), "hci_uart binds this Mac's UART controller");
        assert_eq!(file(&f, "hci_bcm4377.ko"), (true, false));
        assert_eq!(file(&f, "brcmfmac.ko"), (false, true), "Wi-Fi has no firmware yet: still held");
        assert_eq!(run.calls.lock().unwrap().clone(), vec!["modprobe btbcm", "modprobe hci_uart", "modprobe hci_bcm4377"], "dependency order");
        assert!(rep.lines.iter().any(|l| l.contains("hci_uart") && l.contains("given back")), "{:?}", rep.lines);
    }

    #[test]
    fn load_anyway_gives_back_and_loads_in_order_and_is_applied_again_at_the_next_boot() {
        let (f, root) = t2_fs("held-anyway", "Macmini8,1");
        let run = Recorder::default();
        install_pack(&f, &run, "mac-wifi-brcmfmac", "1", "x", "usb", "local", &pack_of("brcm/brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin", &["brcmfmac"])).unwrap();
        run.calls.lock().unwrap().clear();
        let lines = load_bluetooth_anyway(&f, &run);
        assert!(lines.iter().any(|l| l.contains("hci_uart")), "{lines:?}");
        assert_eq!(run.calls.lock().unwrap().clone(), vec!["modprobe btbcm", "modprobe hci_uart", "modprobe hci_bcm4377"]);
        assert!(!crate::t2hold::bluetooth_held(&f.lib_modules) && f.data.join(crate::t2hold::LOAD_ANYWAY_MARK).exists());
        // a second press has nothing left to do
        assert!(load_bluetooth_anyway(&f, &Recorder::default())[0].contains("Nothing was held"));
        // the next boot: the RAM root is fresh and holds the files again; after the data volume is mounted the choice is applied
        hold(&f, &root, "Macmini8,1");
        assert!(crate::t2hold::bluetooth_held(&f.lib_modules));
        let run2 = Recorder::default();
        let log = apply_boot(&f, &run2);
        assert!(!crate::t2hold::bluetooth_held(&f.lib_modules), "{log:?}");
        assert!(run2.calls.lock().unwrap().iter().any(|c| c == "modprobe hci_uart"));
    }

    #[test]
    fn a_failing_modprobe_is_reported_and_other_machines_are_untouched() {
        let (f, _) = t2_fs("held-fail", "Macmini8,1");
        let run = Recorder { fail: Some("modprobe hci_uart".into()), ..Default::default() };
        let lines = load_bluetooth_anyway(&f, &run);
        assert!(lines.iter().any(|l| l.contains("hci_uart did not load")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("hci_bcm4377 loaded")), "the others still load");
        // not a T2 Mac: nothing was set aside, nothing is loaded, no marker
        let (g, _) = t2_fs("held-other", "ThinkPad X1");
        let run = Recorder::default();
        let lines = load_bluetooth_anyway(&g, &run);
        assert_eq!(lines.len(), 1);
        assert!(run.calls.lock().unwrap().is_empty() && !g.data.join(crate::t2hold::LOAD_ANYWAY_MARK).exists());
        assert_eq!(file(&g, "hci_uart.ko"), (true, false));
        // and a Bluetooth pack on such a machine loads only what it asks for
        let run = Recorder::default();
        install_pack(&g, &run, "mac-bt-broadcom", "1", "x", "usb", "local", &pack_of("brcm/BCM.hcd", &["hci_uart"])).unwrap();
        assert_eq!(run.calls.lock().unwrap().clone(), vec!["modprobe hci_uart"]);
    }

    #[test]
    fn a_hcd_without_usb_ids_reloads_the_uart_driver_and_one_with_them_reloads_btusb() {
        let k = crate::known::by_id("mac-bt-broadcom").unwrap();
        let pack_with = |names: &[&str]| {
            let files: Vec<(String, Vec<u8>)> = names.iter().map(|n| (n.to_string(), b"x".to_vec())).collect();
            pack_from_named(k, &files, "t").unwrap().0
        };
        assert_eq!(reload_for(k, &pack_with(&["BCM.hcd"])), vec!["hci_uart"]);
        assert_eq!(reload_for(k, &pack_with(&["BCM4350C0.hcd"])), vec!["hci_uart"]);
        assert_eq!(reload_for(k, &pack_with(&["BCM20702A1-05ac-8289.hcd"])), vec!["btusb"]);
        assert_eq!(reload_for(k, &pack_with(&["BCM.hcd", "brcmbt4377b3-apple,formosa-m.bin"])), vec!["hci_uart", "hci_bcm4377"]);
        // a Wi-Fi board file with a space in its name is accepted by the Wi-Fi entry
        let w = crate::known::by_id("mac-wifi-brcmfmac").unwrap();
        let (p, _) = pack_from_named(w, &[("brcmfmac4350c2-pcie.Apple Inc.-MacBookPro14,1.txt".into(), b"x".to_vec())], "t").unwrap();
        assert_eq!(p.files[0].path, "brcm/brcmfmac4350c2-pcie.Apple Inc.-MacBookPro14,1.txt");
    }
}

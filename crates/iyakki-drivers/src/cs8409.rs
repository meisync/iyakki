//! Sound on the Macs whose HD-audio codec is the Cirrus Logic CS8409 with Apple's amplifiers (MacBookPro13,x and 14,x without the
//! T2 chip, the 2017-2019 iMacs).
//!
//! The kernel's generic codec driver binds that codec and finds no controls (no playback volume, no switch), so the speakers stay
//! silent. The Mac image carries `snd-hda-codec-cs8409.ko` (package `snd-hda-macbookpro`: davidjo's patched copy of the kernel's
//! `patch_cs8409`, GPL-2.0, in `/lib/modules/<ver>/updates`). Its alias `hdaudio:v10138409r*a01*` makes the kernel ask `modprobe`
//! for it when the codec appears, so normally nothing is needed here. This module is the safety net and the explanation:
//!
//! * [`decide`] says what a machine needs: nothing, the driver loaded and the card re-bound, or only the re-bind. It looks at
//!   the DMI model (a short list, see [`MODELS`]), the codec's vendor id and subsystem id (the driver refuses a subsystem id it
//!   does not know, [`SSIDS`]), the driver the codec is bound to and whether the module file and module exist;
//! * [`ensure`] does it: `modprobe snd-hda-codec-cs8409`, then unbinds and binds the PCI sound controller (`snd_hda_intel`), which
//!   makes the kernel build the codec again, now finding the new driver. The codec itself cannot be re-bound alone: the kernel keeps
//!   its "generic" choice for the life of the codec object;
//! * [`missing_line`] is the line the Settings sound page shows when the codec is a CS8409 on one of these Macs without its driver.
//!
//! Everything reads and writes through a [`Roots`] so tests use a folder; the programs started go through [`Runner`].

use std::path::{Path, PathBuf};

use crate::run::Runner;

/// The module (name with underscores, as `/sys/module` and the driver name have it) and its file name.
pub const MODULE: &str = "snd_hda_codec_cs8409";
pub const MODULE_FILE: &str = "snd-hda-codec-cs8409.ko";
/// The codec's vendor id: Cirrus Logic CS8409.
pub const VENDOR: u32 = 0x1013_8409;
/// The generic driver that binds when no specific codec driver is there.
pub const GENERIC_DRIVER: &str = "snd_hda_codec_generic";

/// DMI product names this applies to. From the driver's own source: it names the subsystem ids of the MacBookPro13,1 / 14,1 / 14,2
/// (0x106b3300, 0x106b3600), the MacBookPro14,3 (0x106b3900) and the iMacs 18,1-18,3 and 19,1 (0x106b0e00, 0x0f00, 0x1000).
/// The MacBookPro13,2 and 13,3 are the same generation as 13,1 (Touch Bar and 15 inch) and are listed by the project's issue
/// tracker as working; their subsystem ids are not in the source, so for them the subsystem check below decides.
/// Only the 14,1 was seen by the owner (subsystem 0x106b3300, codec bound to the generic driver).
pub const MODELS: &[&str] = &[
    "MacBookPro13,1",
    "MacBookPro13,2",
    "MacBookPro13,3",
    "MacBookPro14,1",
    "MacBookPro14,2",
    "MacBookPro14,3",
    "iMac18,1",
    "iMac18,2",
    "iMac18,3",
    "iMac19,1",
];

/// The subsystem ids the driver accepts (anything else: "UNKNOWN subsystem id", `-ENODEV`, the generic driver stays).
pub const SSIDS: &[u32] = &[0x106b_3300, 0x106b_3600, 0x106b_3900, 0x106b_0e00, 0x106b_0f00, 0x106b_1000];

/// Where things are. `real()` is the running system; tests point it at a folder.
#[derive(Clone, Debug)]
pub struct Roots {
    pub proc_: PathBuf,
    pub sys: PathBuf,
    pub lib_modules: PathBuf,
}

impl Roots {
    pub fn real() -> Roots {
        Roots { proc_: "/proc".into(), sys: "/sys".into(), lib_modules: "/lib/modules".into() }
    }
    pub fn under(root: &Path) -> Roots {
        Roots { proc_: root.join("proc"), sys: root.join("sys"), lib_modules: root.join("lib/modules") }
    }
    fn product(&self) -> String {
        std::fs::read_to_string(self.sys.join("class/dmi/id/product_name")).map(|s| s.trim().to_string()).unwrap_or_default()
    }
}

/// One HD-audio codec as `/proc/asound/cardN/codec#A` and sysfs describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Codec {
    pub card: u32,
    pub addr: u32,
    pub name: String,
    pub vendor_id: u32,
    pub subsystem_id: u32,
    /// The driver the codec is bound to (`snd_hda_codec_generic`), if any.
    pub driver: Option<String>,
    /// The PCI address of the sound controller of this card (`0000:00:1f.3`).
    pub pci: Option<String>,
}

fn hex_after(line: &str, key: &str) -> Option<u32> {
    let v = line.strip_prefix(key)?.trim().trim_start_matches("0x");
    u32::from_str_radix(v, 16).ok()
}

/// Every codec the kernel lists.
pub fn codecs(r: &Roots) -> Vec<Codec> {
    let mut out = Vec::new();
    let asound = r.proc_.join("asound");
    for e in std::fs::read_dir(&asound).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let Some(card) = n.strip_prefix("card").and_then(|c| c.parse::<u32>().ok()) else { continue };
        for f in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
            let fname = f.file_name().to_string_lossy().into_owned();
            let Some(addr) = fname.strip_prefix("codec#").and_then(|a| a.parse::<u32>().ok()) else { continue };
            let Ok(text) = std::fs::read_to_string(f.path()) else { continue };
            let (mut name, mut vendor, mut ssid) = (String::new(), None, None);
            for l in text.lines() {
                if let Some(v) = l.strip_prefix("Codec:") {
                    name = v.trim().to_string();
                } else if let Some(v) = hex_after(l, "Vendor Id:") {
                    vendor = Some(v);
                } else if let Some(v) = hex_after(l, "Subsystem Id:") {
                    ssid = Some(v);
                }
            }
            let Some(vendor_id) = vendor else { continue };
            let driver = std::fs::read_link(r.sys.join(format!("bus/hdaudio/devices/hdaudioC{card}D{addr}/driver"))).ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()));
            let pci = std::fs::read_link(r.sys.join(format!("class/sound/card{card}/device"))).ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()));
            out.push(Codec { card, addr, name, vendor_id, subsystem_id: ssid.unwrap_or(0), driver, pci });
        }
    }
    out.sort_by_key(|c| (c.card, c.addr));
    out
}

/// Is this model on the list.
pub fn model_applies(product: &str) -> bool {
    MODELS.iter().any(|m| m.eq_ignore_ascii_case(product.trim()))
}

fn find_file(dir: &Path, depth: u32) -> bool {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && depth > 0 {
            if find_file(&e.path(), depth - 1) {
                return true;
            }
        } else if e.file_name().to_string_lossy() == MODULE_FILE {
            return true;
        }
    }
    false
}

/// The module file is in the image (`/lib/modules/<ver>/updates/`, or deeper).
pub fn module_file_present(r: &Roots) -> bool {
    find_file(&r.lib_modules, 3)
}

/// The module is loaded.
pub fn module_loaded(r: &Roots) -> bool {
    r.sys.join("module").join(MODULE).exists()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not one of these Macs, or not this codec.
    NotApplicable,
    /// The codec is bound to the CS8409 driver already.
    Active,
    /// The model is on the list, the codec is a CS8409, but its subsystem id is one the driver does not know.
    UnknownSubsystem,
    /// The driver is needed and the image has no module file.
    ModuleMissing,
    /// Load the module, then re-bind the sound controller.
    LoadAndRebind,
    /// The module is loaded, the codec is still bound to something else (or nothing): re-bind the sound controller.
    Rebind,
}

/// What this codec on this machine needs (a pure function of what was looked at).
pub fn decide(product: &str, c: &Codec, module_file: bool, loaded: bool) -> Verdict {
    if !model_applies(product) || c.vendor_id != VENDOR {
        return Verdict::NotApplicable;
    }
    if c.driver.as_deref() == Some(MODULE) {
        return Verdict::Active;
    }
    if !SSIDS.contains(&c.subsystem_id) {
        return Verdict::UnknownSubsystem;
    }
    if loaded {
        return Verdict::Rebind;
    }
    if !module_file {
        return Verdict::ModuleMissing;
    }
    Verdict::LoadAndRebind
}

/// The first CS8409 codec of a listed Mac with what it needs; `None` on every other machine (the common case: a DMI read only).
pub fn status(r: &Roots) -> Option<(String, Codec, Verdict)> {
    let product = r.product();
    if !model_applies(&product) {
        return None;
    }
    let (file, loaded) = (module_file_present(r), module_loaded(r));
    codecs(r).into_iter().filter(|c| c.vendor_id == VENDOR).map(|c| (decide(&product, &c, file, loaded), c)).next().map(|(v, c)| (product, c, v))
}

/// The line for the Settings sound page, when the speakers cannot work for want of the driver.
pub fn missing_line(r: &Roots) -> Option<String> {
    let (product, c, v) = status(r)?;
    match v {
        Verdict::NotApplicable | Verdict::Active => None,
        Verdict::ModuleMissing => Some(format!("This Mac's speakers need the CS8409 driver (not loaded) · {product} · this image does not carry {MODULE_FILE} · codec {:08x}, subsystem {:08x}", c.vendor_id, c.subsystem_id)),
        Verdict::UnknownSubsystem => Some(format!("This Mac's speakers need the CS8409 driver (not loaded) · {product} · the driver does not know subsystem {:08x}", c.subsystem_id)),
        Verdict::LoadAndRebind | Verdict::Rebind => Some(format!("This Mac's speakers need the CS8409 driver (not loaded) · {product} · codec {:08x}, subsystem {:08x}", c.vendor_id, c.subsystem_id)),
    }
}

/// Writes to sysfs files (a trait so tests record the writes).
pub trait SysWrite {
    fn write(&self, path: &Path, value: &str) -> Result<(), String>;
}

pub struct RealSys;

impl SysWrite for RealSys {
    fn write(&self, path: &Path, value: &str) -> Result<(), String> {
        std::fs::write(path, value).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Records the writes (tests).
#[derive(Default)]
pub struct RecordSys {
    pub writes: std::sync::Mutex<Vec<String>>,
    pub fail: Option<String>,
}

impl SysWrite for RecordSys {
    fn write(&self, path: &Path, value: &str) -> Result<(), String> {
        let line = format!("{} <- {value}", path.display());
        self.writes.lock().unwrap_or_else(|e| e.into_inner()).push(line.clone());
        match &self.fail {
            Some(f) if line.contains(f.as_str()) => Err(format!("{line}: failed")),
            _ => Ok(()),
        }
    }
}

/// Carry out the verdict for one codec: `modprobe` when needed, then unbind and bind the sound controller. Returns the log line.
/// `pause` runs between the unbind and the bind (the real boot sleeps a moment; tests pass a no-op).
pub fn rebind(r: &Roots, run: &dyn Runner, sys: &dyn SysWrite, c: &Codec, v: Verdict, pause: &dyn Fn()) -> Result<String, String> {
    match v {
        Verdict::LoadAndRebind | Verdict::Rebind => {}
        _ => return Err(format!("nothing to do for {v:?}")),
    }
    let Some(pci) = c.pci.as_deref() else { return Err("the sound controller of the card is not known (no /sys/class/sound/cardN/device)".into()) };
    if v == Verdict::LoadAndRebind {
        run.run("modprobe", &["snd-hda-codec-cs8409"])?;
    }
    let drv = r.sys.join("bus/pci/drivers/snd_hda_intel");
    sys.write(&drv.join("unbind"), pci)?;
    pause();
    sys.write(&drv.join("bind"), pci)?;
    Ok(format!("CS8409 sound: {} (codec {:08x}, subsystem {:08x}) re-bound through {} with {MODULE}", c.name, c.vendor_id, c.subsystem_id, pci))
}

/// The boot step: for a listed Mac with a CS8409 codec bound to the wrong driver, load the driver and re-bind the card; one log line.
/// `None` when the machine needs nothing (the usual case). The caller retries while the card is still being probed.
pub fn ensure(r: &Roots, run: &dyn Runner, sys: &dyn SysWrite, pause: &dyn Fn()) -> Option<String> {
    let (product, c, v) = status(r)?;
    match v {
        Verdict::NotApplicable => None,
        Verdict::Active => Some(format!("CS8409 sound: {product}: the codec uses {MODULE}")),
        Verdict::ModuleMissing => Some(format!("CS8409 sound: {product}: the image has no {MODULE_FILE}; the speakers stay silent")),
        Verdict::UnknownSubsystem => Some(format!("CS8409 sound: {product}: subsystem {:08x} is not one the driver knows; the generic driver stays", c.subsystem_id)),
        Verdict::LoadAndRebind | Verdict::Rebind => Some(match rebind(r, run, sys, &c, v, pause) {
            Ok(l) => l,
            Err(e) => format!("CS8409 sound: {product}: could not load the driver and re-bind the card: {e}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::Recorder;

    fn tmp(tag: &str) -> PathBuf {
        let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-cs8409-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// The MacBookPro14,1 as seen over SSH: codec "Cirrus Logic Generic", vendor 0x10138409, subsystem 0x106b3300, bound to the
    /// generic driver, controller 0000:00:1f.3.
    fn macbook(tag: &str, product: &str, driver: Option<&str>, module_file: bool, loaded: bool) -> (PathBuf, Roots) {
        let root = tmp(tag);
        let r = Roots::under(&root);
        put(&r.sys, "class/dmi/id/product_name", &format!("{product}\n"));
        put(&r.proc_, "asound/card0/codec#0", "Codec: Cirrus Logic Generic\nAddress: 0\nAFG Function Id: 0x1 (unsol 1)\nVendor Id: 0x10138409\nSubsystem Id: 0x106b3300\nRevision Id: 0x100100\n");
        std::fs::create_dir_all(r.sys.join("devices/pci0000:00/0000:00:1f.3")).unwrap();
        std::fs::create_dir_all(r.sys.join("class/sound")).unwrap();
        std::fs::create_dir_all(r.sys.join("class/sound/card0")).unwrap();
        std::os::unix::fs::symlink("../../../devices/pci0000:00/0000:00:1f.3", r.sys.join("class/sound/card0/device")).unwrap();
        if let Some(d) = driver {
            std::fs::create_dir_all(r.sys.join(format!("bus/hdaudio/drivers/{d}"))).unwrap();
            std::fs::create_dir_all(r.sys.join("bus/hdaudio/devices/hdaudioC0D0")).unwrap();
            std::os::unix::fs::symlink(format!("../../../../bus/hdaudio/drivers/{d}"), r.sys.join("bus/hdaudio/devices/hdaudioC0D0/driver")).unwrap();
        }
        if module_file {
            put(&r.lib_modules, "6.12.112/updates/snd-hda-codec-cs8409.ko", "x");
        }
        if loaded {
            std::fs::create_dir_all(r.sys.join("module").join(MODULE)).unwrap();
        }
        (root, r)
    }

    #[test]
    fn the_codec_of_the_macbookpro_14_1_is_read_with_its_driver_and_controller() {
        let (root, r) = macbook("read", "MacBookPro14,1", Some(GENERIC_DRIVER), false, false);
        let c = codecs(&r);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].card, c[0].addr, c[0].vendor_id, c[0].subsystem_id), (0, 0, 0x1013_8409, 0x106b_3300));
        assert_eq!(c[0].name, "Cirrus Logic Generic");
        assert_eq!(c[0].driver.as_deref(), Some("snd_hda_codec_generic"));
        assert_eq!(c[0].pci.as_deref(), Some("0000:00:1f.3"));
        let _ = std::fs::remove_dir_all(root);
    }

    fn codec(vendor: u32, ssid: u32, driver: Option<&str>) -> Codec {
        Codec { card: 0, addr: 0, name: "x".into(), vendor_id: vendor, subsystem_id: ssid, driver: driver.map(String::from), pci: Some("0000:00:1f.3".into()) }
    }

    #[test]
    fn the_decision_table_by_model_codec_driver_and_module() {
        let g = Some(GENERIC_DRIVER);
        // the owner's machine: generic driver, module in the image, not loaded -> load and re-bind
        assert_eq!(decide("MacBookPro14,1", &codec(VENDOR, 0x106b_3300, g), true, false), Verdict::LoadAndRebind);
        // no module file in the image
        assert_eq!(decide("MacBookPro14,1", &codec(VENDOR, 0x106b_3300, g), false, false), Verdict::ModuleMissing);
        // module loaded but the codec was bound before: only re-bind
        assert_eq!(decide("MacBookPro14,1", &codec(VENDOR, 0x106b_3300, g), true, true), Verdict::Rebind);
        assert_eq!(decide("MacBookPro14,1", &codec(VENDOR, 0x106b_3300, None), true, true), Verdict::Rebind, "no driver at all");
        // already the CS8409 driver
        assert_eq!(decide("MacBookPro14,1", &codec(VENDOR, 0x106b_3300, Some(MODULE)), true, true), Verdict::Active);
        // the driver does not know this subsystem id
        assert_eq!(decide("MacBookPro14,1", &codec(VENDOR, 0x1234_5678, g), true, false), Verdict::UnknownSubsystem);
        // not a listed model (a Dell with a CS8409, a Mac mini, a T2 MacBook), or another codec
        assert_eq!(decide("Latitude 5520", &codec(VENDOR, 0x1028_0a11, g), true, false), Verdict::NotApplicable);
        assert_eq!(decide("Macmini8,1", &codec(VENDOR, 0x106b_3300, g), true, false), Verdict::NotApplicable);
        assert_eq!(decide("MacBookPro15,1", &codec(VENDOR, 0x106b_3300, g), true, false), Verdict::NotApplicable);
        assert_eq!(decide("MacBookPro14,1", &codec(0x10ec_0269, 0x106b_3300, g), true, false), Verdict::NotApplicable);
        // every listed model is accepted, with spaces and case ignored
        for m in MODELS {
            assert!(model_applies(&format!(" {} \n", m.to_ascii_lowercase())), "{m}");
        }
        assert!(!model_applies("MacBookPro12,1") && !model_applies("MacBookPro15,2") && !model_applies(""));
    }

    #[test]
    fn the_list_has_no_t2_mac() {
        for m in MODELS {
            assert!(!crate::macboard::is_t2(m), "{m} has the T2 chip (apple-bce handles those)");
        }
    }

    #[test]
    fn the_settings_line_names_the_missing_driver_and_the_model_and_is_silent_when_all_is_well() {
        let (root, r) = macbook("line", "MacBookPro14,1", Some(GENERIC_DRIVER), true, false);
        let l = missing_line(&r).unwrap();
        assert!(l.starts_with("This Mac's speakers need the CS8409 driver (not loaded)") && l.contains("MacBookPro14,1") && l.contains("106b3300"), "{l}");
        let _ = std::fs::remove_dir_all(root);
        let (root, r) = macbook("line-no-file", "MacBookPro14,1", Some(GENERIC_DRIVER), false, false);
        let l = missing_line(&r).unwrap();
        assert!(l.starts_with("This Mac's speakers need the CS8409 driver (not loaded)") && l.contains("does not carry"), "{l}");
        let _ = std::fs::remove_dir_all(root);
        let (root, r) = macbook("line-ok", "MacBookPro14,1", Some(MODULE), true, true);
        assert_eq!(missing_line(&r), None);
        let _ = std::fs::remove_dir_all(root);
        let (root, r) = macbook("line-other", "Macmini8,1", Some(GENERIC_DRIVER), true, false);
        assert_eq!(missing_line(&r), None, "another Mac: nothing said");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_rebind_loads_the_module_then_unbinds_and_binds_the_controller_in_that_order() {
        let (root, r) = macbook("rebind", "MacBookPro14,1", Some(GENERIC_DRIVER), true, false);
        let (run, sys) = (Recorder::default(), RecordSys::default());
        let paused = std::sync::atomic::AtomicU32::new(0);
        let log = ensure(&r, &run, &sys, &|| {
            paused.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        assert!(log.starts_with("CS8409 sound:") && log.contains("0000:00:1f.3") && log.contains("snd_hda_codec_cs8409"), "{log}");
        assert_eq!(*run.calls.lock().unwrap(), vec!["modprobe snd-hda-codec-cs8409".to_string()]);
        let w = sys.writes.lock().unwrap().clone();
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].ends_with("bus/pci/drivers/snd_hda_intel/unbind <- 0000:00:1f.3"), "{w:?}");
        assert!(w[1].ends_with("bus/pci/drivers/snd_hda_intel/bind <- 0000:00:1f.3"), "{w:?}");
        assert_eq!(paused.load(std::sync::atomic::Ordering::SeqCst), 1, "a pause between unbind and bind");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_module_that_is_loaded_already_is_not_loaded_again_only_the_card_is_rebound() {
        let (root, r) = macbook("rebind-only", "MacBookPro14,1", Some(GENERIC_DRIVER), true, true);
        let (run, sys) = (Recorder::default(), RecordSys::default());
        assert!(ensure(&r, &run, &sys, &|| {}).unwrap().contains("re-bound"));
        assert!(run.calls.lock().unwrap().is_empty());
        assert_eq!(sys.writes.lock().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nothing_is_touched_when_the_driver_is_active_or_missing_or_the_mac_is_another() {
        for (tag, product, driver, file, loaded, say) in [
            ("active", "MacBookPro14,1", Some(MODULE), true, true, Some("uses")),
            ("nofile", "MacBookPro14,1", Some(GENERIC_DRIVER), false, false, Some("no snd-hda-codec-cs8409.ko")),
            ("other", "Macmini8,1", Some(GENERIC_DRIVER), true, false, None),
            ("pc", "ThinkPad X1", Some(GENERIC_DRIVER), true, false, None),
        ] {
            let (root, r) = macbook(tag, product, driver, file, loaded);
            let (run, sys) = (Recorder::default(), RecordSys::default());
            let log = ensure(&r, &run, &sys, &|| {});
            match say {
                Some(s) => assert!(log.as_deref().unwrap_or("").contains(s), "{tag}: {log:?}"),
                None => assert_eq!(log, None, "{tag}"),
            }
            assert!(run.calls.lock().unwrap().is_empty() && sys.writes.lock().unwrap().is_empty(), "{tag}");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn a_failing_modprobe_or_unbind_is_reported_in_the_log_line_not_hidden() {
        let (root, r) = macbook("fail1", "MacBookPro14,1", Some(GENERIC_DRIVER), true, false);
        let (run, sys) = (Recorder { fail: Some("modprobe".into()), ..Default::default() }, RecordSys::default());
        let l = ensure(&r, &run, &sys, &|| {}).unwrap();
        assert!(l.contains("could not load the driver") && l.contains("modprobe"), "{l}");
        assert!(sys.writes.lock().unwrap().is_empty(), "the card is not unbound when the module did not load");
        let _ = std::fs::remove_dir_all(root);
        let (root, r) = macbook("fail2", "MacBookPro14,1", Some(GENERIC_DRIVER), true, true);
        let (run, sys) = (Recorder::default(), RecordSys { fail: Some("unbind".into()), ..Default::default() });
        let l = ensure(&r, &run, &sys, &|| {}).unwrap();
        assert!(l.contains("could not load the driver") && l.contains("unbind"), "{l}");
        assert_eq!(sys.writes.lock().unwrap().len(), 1, "no bind after a failed unbind");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_card_without_a_known_controller_is_not_unbound() {
        let (root, r) = macbook("nopci", "MacBookPro14,1", Some(GENERIC_DRIVER), true, true);
        let _ = std::fs::remove_file(r.sys.join("class/sound/card0/device"));
        let (run, sys) = (Recorder::default(), RecordSys::default());
        let l = ensure(&r, &run, &sys, &|| {}).unwrap();
        assert!(l.contains("not known") && sys.writes.lock().unwrap().is_empty(), "{l}");
        let _ = std::fs::remove_dir_all(root);
    }
}

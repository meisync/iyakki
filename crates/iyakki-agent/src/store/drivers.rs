//! Driver packs in the store: the `extension` package of `ext_kind: driver` (target `system`).
//!
//! A driver pack is data only (see `iyakki-drivers`): firmware, modprobe options, kernel option suggestions and, from a
//! trusted publisher and only for the exact running kernel, prebuilt modules. This file connects that to the store:
//! the review lines, install and remove on the device, the "Drivers for this machine" overview, building a local pack
//! from a folder or USB stick or from Apple's camera driver file, the built-in catalog entries (metadata and
//! instructions, no blobs) and the assistant's `driver_status` and `driver_install`.

use std::path::Path;

use iyakki_drivers::{apply, facetime, known, run, Availability, Delivery, Fs, Machine, Pack, Roots, Row};
use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use super::builtin::{iyakki, Built, VERSION};
use super::catalog::{sha256_hex, Action, Entry};
use super::install::{Actor, Confirm};
use super::manifest::{Meta, Publisher};
use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;
use crate::services::Services;
use crate::skills::Scope;

const ADDED: i64 = 1_790_000_000;
/// Largest Apple driver file we read (the real one is about a megabyte).
pub const MAX_DRIVER_FILE: u64 = 16 * 1024 * 1024;

/// The drivers of one machine: where the files go and which sysfs/proc trees to read.
#[derive(Clone)]
pub struct Drivers {
    pub fs: Fs,
    pub roots: Roots,
    /// Is this the device image (real `/lib/firmware`), or a sandbox folder?
    pub real: bool,
}

impl Drivers {
    pub fn for_data(data: &Path) -> Drivers {
        let (fs, real) = Fs::for_data(data);
        Drivers { fs, roots: Roots::real(), real }
    }

    pub fn with_roots(mut self, roots: Roots) -> Drivers {
        self.roots = roots;
        self
    }

    pub fn for_services(svc: &Services) -> Drivers {
        Drivers::for_data(svc.dir())
    }

    fn runner(&self) -> Box<dyn run::Runner> {
        if self.real { Box::new(run::System) } else { Box::new(run::Dry) }
    }

    pub fn machine(&self) -> Machine {
        Machine::collect(&self.roots)
    }

    /// The "Drivers for this machine" list. Empty for a machine none of the entries applies to.
    pub fn rows(&self) -> Vec<Row> {
        known::for_machine(&self.machine(), &|p| apply::firmware_present(&self.fs, p), &apply::active_ids(&self.fs))
    }

    pub fn installed(&self) -> Vec<apply::Record> {
        apply::installed(&self.fs)
    }

    /// What the review of a driver pack shows, or the reason it must be refused.
    pub fn review_bits(&self, payload: &str, level: &str, admin: bool, shared: bool) -> Result<Bits, String> {
        let p = Pack::parse(payload)?;
        if !admin {
            return Err("Only administrators can install drivers: they change the whole machine.".into());
        }
        if !shared {
            return Err("Drivers are installed for the whole machine, not for one account.".into());
        }
        let mut bits = Bits::default();
        let m = self.machine();
        if p.has_modules() {
            if !apply::level_may_bring_modules(level) {
                return Err(format!("This pack carries kernel modules, which run inside the kernel. They are accepted only from Official, Verified or Trusted publishers; this pack is {level}."));
            }
            let reference = self.fs.reference_vermagic();
            for k in &p.modules {
                if k.kernel != m.release {
                    return Err(format!("Module {} is for kernel {} but this system runs {}. A module only loads into the exact kernel it was built for.", k.name, k.kernel, if m.release.is_empty() { "an unknown release" } else { &m.release }));
                }
                // the module's own record of the kernel it was built for, not just what the pack says about it
                let ko = iyakki_drivers::b64::decode(&k.data).ok_or_else(|| format!("Module {} is not valid base64.", k.name))?;
                let info = iyakki_drivers::modinfo::check(&ko, &m.release, reference.as_deref()).map_err(|e| format!("Module {}: {e}", k.name))?;
                if !info.gpl_compatible() {
                    bits.warnings.push(format!("Module {} is NON-FREE software (licence \"{}\"). Loading it marks the kernel as tainted, and nobody outside its vendor can inspect all of it. Install it only if you built it yourself or trust who did.", k.name, info.license));
                }
            }
            bits.warnings.push("The pack brings kernel modules: they run with full power inside the kernel. A module that is not GPL-licensed also marks the kernel as tainted.".into());
        }
        if !p.matches.is_empty() && m.first_match(&p.matches).is_none() {
            bits.warnings.push("Nothing in this machine matches what the pack says it is for. Install it only if you know why.".into());
        }
        bits.permissions = vec!["write:/lib/firmware".into()];
        if !p.modprobe.is_empty() {
            bits.permissions.push("write:/etc/modprobe.d".into());
        }
        if !p.reload.is_empty() || !p.load.is_empty() {
            bits.permissions.push("run:modprobe".into());
        }
        if p.has_modules() {
            bits.permissions.push("run:insmod".into());
        }
        for f in &p.files {
            bits.preview.push(format!("firmware file: {} ({} bytes)", f.path, f.data.len() / 4 * 3));
        }
        for l in &p.modprobe {
            bits.preview.push(format!("module option: {l}"));
        }
        for k in &p.modules {
            bits.preview.push(format!("kernel module: {}.ko for kernel {}", k.name, k.kernel));
        }
        if !p.load.is_empty() || !p.reload.is_empty() {
            bits.preview.push(format!("then loads again: {}", p.load.iter().chain(&p.reload).cloned().collect::<Vec<_>>().join(", ")));
        }
        for c in &p.cmdline {
            bits.preview.push(format!("suggested kernel option (not applied): {c}"));
        }
        if !p.why.is_empty() {
            bits.preview.push(p.why.clone());
        }
        Ok(bits)
    }

    pub fn install(&self, id: &str, version: &str, license: &str, source: &str, level: &str, payload: &str) -> Result<apply::Report, String> {
        let p = Pack::parse(payload)?;
        apply::install_pack(&self.fs, self.runner().as_ref(), id, version, license, source, level, &p)
    }

    pub fn remove(&self, id: &str) -> Result<apply::Report, String> {
        apply::remove_pack(&self.fs, id)
    }

    /// Build an unsigned local package for the entry `id` from the files in `dir` (a folder or a mounted USB stick).
    /// Returns the package text, ready for the store's review, and what was taken from the folder.
    pub fn folder_package(&self, id: &str, dir: &Path, now: i64) -> Result<(String, Vec<String>), String> {
        let k = known::by_id(id).ok_or_else(|| format!("`{id}` is not a driver the store knows"))?;
        // a signed pack someone built for this entry (the Wi-Fi driver) goes to the review as it is: its signature decides the trust
        if let Some((text, label)) = signed_pack_in(dir, k.id)? {
            return Ok((text, vec![label]));
        }
        if k.accept.is_empty() {
            return Err(format!("no signed driver pack for {} (a .iyp file named for {}) was found in {}. {}", k.title, k.id, dir.display(), k.supply));
        }
        let (pack, taken) = apply::pack_from_folder(k, dir)?;
        Ok((local_package(k, &pack, now, "Files you supplied from a folder or USB stick. They are not signed, so the review warns you: install them only if you got them from a source you trust.")?, taken))
    }

    /// The camera: extract the firmware from Apple's `AppleCameraInterface` file. `accepted` is the person's answer to
    /// the licence notice; without it nothing is read.
    pub fn facetime_package(&self, driver_file: &[u8], accepted: bool, now: i64) -> Result<(String, String), String> {
        if !accepted {
            return Err(format!("Not done: {} Say that you accept to go on.", facetime::NOTICE));
        }
        let e = facetime::extract(driver_file)?;
        let k = known::by_id("mac-facetimehd").ok_or("camera entry missing")?;
        let text = local_package(k, &facetime::pack_for(&e), now, "Firmware extracted on this computer from the Apple driver file you supplied.")?;
        Ok((text, format!("firmware {} from {} (sha256 {})", e.version, e.from, &e.sha256[..16])))
    }
}

/// Look (two levels deep, no symlinks) for signed `.iyp` packages of the entry `id`; the newest version wins. Nothing is
/// installed here: the text goes to the normal review, which checks the signature chain.
fn signed_pack_in(dir: &Path, id: &str) -> Result<Option<(String, String)>, String> {
    let mut best: Option<(String, String, String)> = None;
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((d, depth)) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for e in rd.flatten() {
            let (p, Ok(t)) = (e.path(), e.file_type()) else { continue };
            if t.is_symlink() {
                continue;
            }
            if t.is_dir() && depth < 2 {
                stack.push((p, depth + 1));
            } else if t.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("iyp")) && e.metadata().is_ok_and(|m| m.len() as usize <= crate::pkg::MAX_EXTENSION_PACKAGE_BYTES) {
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                let Ok(pk) = crate::pkg::parse(&text) else { continue };
                if pk.manifest.kind != "extension" || pk.manifest.name != id {
                    continue;
                }
                let v = pk.manifest.version.clone();
                if best.as_ref().is_none_or(|(_, bv, _)| crate::pkg::newer(&v, bv)) {
                    let signed = if pk.manifest.signer.is_some() { "signed" } else { "not signed" };
                    best = Some((text, v.clone(), format!("{} (driver pack {id} {v}, {signed}, sha256 {})", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), pk.manifest.sha256.get(..16).unwrap_or(""))));
                }
            }
        }
    }
    Ok(best.map(|(t, _, l)| (t, l)))
}

#[derive(Default, Clone, Debug)]
pub struct Bits {
    pub preview: Vec<String>,
    pub warnings: Vec<String>,
    pub permissions: Vec<String>,
}

fn driver_meta(k: &known::Known, extra: &str) -> Meta {
    let availability = k.availability.label();
    Meta {
        title: k.title.chars().take(60).collect(),
        summary: k.summary.chars().take(140).collect(),
        description: format!("{}\n\nStatus: {availability}.\nLicence: {}.\nWhat you supply: {}{}{}", k.why, k.license, k.supply, if extra.is_empty() { "" } else { "\n\n" }, extra),
        publisher: iyakki(),
        categories: vec!["drivers".into()],
        tags: vec!["mac".into(), "hardware".into()],
        target: "system".into(),
        ext_kind: "driver".into(),
        permissions: vec!["write:/lib/firmware".into()],
        ..Default::default()
    }
}

/// An unsigned package for a pack made on this device.
fn local_package(k: &known::Known, pack: &Pack, now: i64, extra: &str) -> Result<String, String> {
    let mut meta = driver_meta(k, extra);
    meta.publisher = Publisher { id: "local".into(), name: "This computer".into(), url: String::new() };
    meta.permissions = vec!["write:/lib/firmware".into(), "run:modprobe".into()];
    // versions only go up (day, then minute of the day: segments are at most six digits); a re-import a minute later is newer
    let version = format!("1.{}.{}", (now / 86_400).max(1), (now % 86_400) / 60);
    crate::pkg::build_generic("extension", k.id, &version, "LicenseRef-user-supplied", &pack.to_json(), Some(&meta.to_json()), None, None).map_err(|e| e.to_string())
}

/// The built-in catalog entries: one per known driver. The keyboard-options pack is a real, installable package; the others
/// are instructions that lead to the Drivers view (they need files only the person can supply, or a different image).
pub fn catalog_items() -> Vec<Built> {
    known::KNOWN
        .iter()
        .map(|k| {
            let meta = driver_meta(k, "");
            let mut e = Entry { id: k.id.into(), kind: "extension".into(), name: k.id.into(), version: VERSION.into(), license: k.license.split([' ', '(']).next().unwrap_or("").trim_matches(':').to_string(), added: ADDED - 86_400 * 7, meta, ..Default::default() };
            if e.license.is_empty() || !e.license.chars().all(|c| c.is_ascii_alphanumeric() || "-.+".contains(c)) {
                e.license = "Apache-2.0".into();
            }
            let mut package = None;
            if k.delivery == Delivery::Builtin {
                let pack = Pack {
                    v: 1,
                    matches: k.match_rules(),
                    modprobe: vec!["options hid_apple fnmode=2".into()],
                    load: vec!["hid_apple".into()],
                    why: k.why.into(),
                    ..Default::default()
                };
                let mut meta = e.meta.clone();
                meta.permissions = vec!["write:/etc/modprobe.d".into(), "run:modprobe".into()];
                e.meta = meta;
                e.license = "Apache-2.0".into();
                let text = crate::pkg::build_generic("extension", k.id, VERSION, "Apache-2.0", &pack.to_json(), Some(&e.meta.to_json()), None, None).expect("a shipped driver pack packs");
                e.size = text.len() as u64;
                e.sha256 = sha256_hex(text.as_bytes());
                e.builtin = k.id.into();
                package = Some(text);
            } else {
                e.action = Some(Action { open: "drivers".into(), id: k.id.into() });
                e.license = "-".into();
            }
            e.featured = matches!(k.id, "mac-bt-broadcom" | "mac-facetimehd");
            Built { entry: e, package }
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ assistant tools

pub fn is_driver_tool(name: &str) -> bool {
    matches!(name, "driver_status" | "driver_install")
}

pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !p.apps.contains(&App::Store) {
        return vec![];
    }
    let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    vec![
        ToolDef {
            spec: ToolSpec {
                name: "driver_status".into(),
                description: "What hardware on this machine is missing a driver or firmware, from detection (DMI product, PCI and USB ids, loaded modules, firmware files): for each known device its status (driver loaded, firmware missing, no driver), whether it works with the standard image, after a driver pack, only with the Mac image variant, or not at all, and the installed driver packs. Use it when the person says Wi-Fi, Bluetooth, camera, audio or fans do not work on a Mac or other bare-metal machine. For the Broadcom BCM4360 Wi-Fi of the Mac mini 2014 it explains the options: a non-free driver pack the person builds themselves (never shipped, taints the kernel), a USB Wi-Fi adapter with an open driver, or Ethernet. Read-only.".into(),
                schema: obj(json!({}), &[]),
            },
            risk: Risk::Safe,
            untrusted: false,
        },
        ToolDef {
            spec: ToolSpec {
                name: "driver_install".into(),
                description: "Install a driver pack for this machine. `pack` is an id from driver_status (such as mac-bt-broadcom). `source` is where the person's files are: a folder or USB stick (firmware files, for example the Bluetooth .hcd) or, for mac-facetimehd, the path of Apple's AppleCameraInterface file (then `accept_licence` must be true after the person read Apple's licence notice). For mac-wifi-bcm4360 `source` is the folder or USB stick holding the signed pack (.iyp) the person built with dist/drivers/build-wl-pack.sh; it installs only if its publisher key is trusted (STORE, Settings). Packs without files to supply (mac-keyboard-options) need no source. This changes the system and ALWAYS asks, even in Auto mode. Nothing is compiled on the device; kernel modules are only accepted from trusted publishers and only for the exact running kernel.".into(),
                schema: obj(json!({"pack": {"type": "string"}, "source": {"type": "string"}, "accept_licence": {"type": "boolean"}}), &["pack"]),
            },
            risk: Risk::Ask,
            untrusted: false,
        },
    ]
}

fn status_text(d: &Drivers) -> String {
    let m = d.machine();
    let rows = d.rows();
    let mut out = format!("Machine: {} {} (kernel {}).\n", if m.vendor.is_empty() { "unknown" } else { &m.vendor }, if m.product.is_empty() { "(no DMI)" } else { &m.product }, if m.release.is_empty() { "?" } else { &m.release });
    if rows.is_empty() {
        out.push_str("No driver packs apply to this machine (the built-in drivers list covers Macs only).\n");
    }
    for r in &rows {
        out.push_str(&format!("- {} [{}]: {} ({}){}\n", r.known.id, r.known.availability.label(), r.status.label(), r.detail, if r.installed { ", pack installed" } else { "" }));
        for dev in r.devices.iter().take(3) {
            out.push_str(&format!("    {dev}\n"));
        }
        if r.known.availability != Availability::Standard && r.status != known::Status::Working {
            out.push_str(&format!("    needs: {}\n", r.known.supply));
        }
    }
    let inst = d.installed();
    if !inst.is_empty() {
        out.push_str(&format!("Installed packs: {}.\n", inst.iter().map(|r| format!("{} {}{}", r.id, r.version, if apply::is_disabled(&d.fs, &r.id) { " (off)" } else { "" })).collect::<Vec<_>>().join(", ")));
    }
    out
}

pub fn describe(svc: &Services, name: &str, args: &Value) -> Option<String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("");
    match name {
        "driver_status" => Some("check which devices on this machine lack a driver or firmware".into()),
        "driver_install" => {
            let _ = svc;
            let title = known::by_id(s("pack")).map_or(s("pack").to_string(), |k| k.title.to_string());
            Some(format!("install the driver pack “{title}”{}: it writes firmware into /lib/firmware (links to the data volume), may reload drivers{}. You will see the full review.", if s("source").is_empty() { String::new() } else { format!(" from {}", s("source")) }, if args.get("accept_licence").and_then(Value::as_bool) == Some(true) { " and extracts firmware from Apple's file under Apple's licence" } else { "" }))
        }
        _ => None,
    }
}

pub fn call(name: &str, args: &Value, svc: &Services, p: &Policy, user: &str) -> Result<String, String> {
    if !p.apps.contains(&App::Store) {
        return Err("your account may not use the store".into());
    }
    let d = Drivers::for_services(svc);
    match name {
        "driver_status" => Ok(status_text(&d)),
        "driver_install" => install_call(&d, args, svc, p, user),
        _ => Err(format!("unknown tool `{name}`")),
    }
}

fn install_call(d: &Drivers, args: &Value, svc: &Services, p: &Policy, user: &str) -> Result<String, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    let id = s("pack").ok_or("give the pack id from driver_status")?;
    let k = known::by_id(id).ok_or_else(|| format!("`{id}` is not a driver pack the store knows. Use driver_status for the list."))?;
    let st = super::Store::for_services(svc);
    let a = Actor::from_policy(user, p);
    let now = st.now();
    let bytes = match k.delivery {
        Delivery::Info => return Err(format!("{} cannot be installed: {} {}", k.title, k.availability.label(), k.why)),
        Delivery::Builtin => {
            let loaded = st.local_catalogs(svc);
            let items = st.items(svc, &a, &loaded);
            let i = items.iter().find(|i| i.entry.id == k.id).ok_or("the entry is not in the catalog")?;
            if matches!(i.state, super::State::Installed) {
                return Err("It is already installed.".into());
            }
            let fetch = super::catalog::HttpFetch(svc.quick_upstream());
            let known_names = super::Store::known_names(&items);
            let r = st.review_item(svc, &a, i, &loaded, &fetch, Scope::Shared, &known_names)?;
            if let Some(why) = &r.refusal {
                return Err(format!("Not installed: {why}"));
            }
            let rec = st.install(svc, &a, &r, Confirm::all()).map_err(|e| format!("Not installed: {e}"))?;
            return Ok(format!("Installed {} {}. {}", rec.title, rec.version, k.availability.label()));
        }
        Delivery::Folder => {
            let src = s("source").ok_or_else(|| format!("give the folder or USB stick with the files. {}", k.supply))?;
            d.folder_package(k.id, Path::new(src), now)?.0.into_bytes()
        }
        Delivery::Extract => {
            let src = s("source").ok_or_else(|| format!("give the path of Apple's AppleCameraInterface file. {}", k.supply))?;
            let accepted = args.get("accept_licence").and_then(Value::as_bool) == Some(true);
            if !accepted {
                return Err(format!("Not done: {} Ask the person, then call again with accept_licence true.", facetime::NOTICE));
            }
            let meta = std::fs::metadata(src).map_err(|e| format!("{src}: {e}"))?;
            if !meta.is_file() || meta.len() > MAX_DRIVER_FILE {
                return Err(format!("{src} is not a file of a plausible size for the Apple driver"));
            }
            let data = std::fs::read(src).map_err(|e| format!("{src}: {e}"))?;
            d.facetime_package(&data, true, now)?.0.into_bytes()
        }
    };
    let known_names = vec![];
    let r = st.review_sideload(svc, &a, bytes, Scope::Shared, &known_names);
    if let Some(why) = &r.refusal {
        return Err(format!("Not installed: {why}"));
    }
    let rec = st.install(svc, &a, &r, Confirm::all()).map_err(|e| format!("Not installed: {e}"))?;
    Ok(format!("Installed {} {} for this machine ({}). {}", rec.title, rec.version, r.verdict.level.label(), if k.availability == Availability::NeedsMacImage { "The kernel of this image has no driver for it yet: it needs the Mac image variant." } else { "The driver was asked to reload; check driver_status." }))
}

#[cfg(test)]
#[path = "drivers_tests.rs"]
mod tests;

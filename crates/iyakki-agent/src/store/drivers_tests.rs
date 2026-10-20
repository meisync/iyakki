//! Driver packs through the store: the built-in entries, review and install of a pack (signed, local, with modules),
//! removal, the "Drivers for this machine" status text, the extraction guard and the assistant tools.

use iyakki_drivers::{b64, modinfo, pack};
use serde_json::json;

use super::super::install::tests::{admin, fx, user, Fx, NOW};
use super::super::install::Confirm;
use super::super::trust::Level;
use super::super::{builtin, State, Store};
use super::*;
use crate::pkg::{self, new_signing_key};

fn items(f: &Fx, a: &Actor) -> Vec<super::super::Item> {
    let l = f.store.local_catalogs(&f.svc);
    f.store.items(&f.svc, a, &l)
}

fn sandbox(f: &Fx) -> Drivers {
    Drivers::for_data(&f.dir)
}

fn stick(f: &Fx, files: &[(&str, &[u8])]) -> std::path::PathBuf {
    let d = f.dir.join("stick");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for (n, b) in files {
        std::fs::write(d.join(n), b).unwrap();
    }
    d
}

fn signed_pack(f: &Fx, name: &str, version: &str, p: &Pack, key: &pkg::SigningKey) -> Vec<u8> {
    let meta = Meta { title: "Test driver".into(), summary: "A test".into(), publisher: iyakki(), target: "system".into(), ext_kind: "driver".into(), permissions: vec!["write:/lib/firmware".into()], ..Default::default() };
    let _ = f;
    pkg::build_generic("extension", name, version, "GPL-2.0", &p.to_json(), Some(&meta.to_json()), None, Some(key)).unwrap().into_bytes()
}

fn fw_pack() -> Pack {
    Pack { v: 1, files: vec![pack::FwFile { path: "brcm/test.bin".into(), data: b64::encode(b"FW"), sha256: String::new() }], modprobe: vec!["options hid_apple fnmode=2".into()], ..Default::default() }
}

#[test]
fn the_built_in_catalog_lists_every_known_driver_as_a_driver_extension_for_the_system() {
    let f = fx("drv-catalog");
    let v = items(&f, &admin());
    for k in iyakki_drivers::KNOWN {
        let i = v.iter().find(|i| i.entry.id == k.id).unwrap_or_else(|| panic!("{} is listed", k.id));
        assert_eq!((i.entry.kind.as_str(), i.entry.meta.ext_kind.as_str(), i.entry.meta.target.as_str()), ("extension", "driver", "system"), "{}", k.id);
        assert!(i.entry.meta.description.contains(k.availability.label()) && i.entry.meta.description.contains("What you supply"), "{}", k.id);
        assert_eq!(super::super::manifest::extension_of("extension", &i.entry.meta), Some(("system".into(), "driver".into())));
        if k.delivery == Delivery::Builtin {
            assert_eq!((i.verdict.level, i.state.clone()), (Level::Official, State::Available));
        } else {
            assert_eq!(i.state, State::Link("drivers".into()), "{}: needs files only the person can supply", k.id);
        }
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_keyboard_options_pack_installs_from_the_store_writes_its_options_and_removes_cleanly() {
    let f = fx("drv-kbd");
    let a = admin();
    let l = f.store.local_catalogs(&f.svc);
    let v = items(&f, &a);
    let it = v.iter().find(|i| i.entry.id == "mac-keyboard-options").unwrap();
    let r = f.store.review_item(&f.svc, &a, it, &l, &super::super::catalog::tests::FakeFetch::default(), Scope::Shared, &[]).unwrap();
    assert!(r.refusal.is_none(), "{}", r.summary());
    assert!(r.permissions.iter().any(|p| p.words.contains("/etc/modprobe.d")) && r.permissions.iter().any(|p| p.words.contains("modprobe")), "{:?}", r.permissions);
    assert!(r.preview.iter().any(|l| l.contains("options hid_apple fnmode=2")));
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    let d = sandbox(&f);
    assert_eq!(d.installed().len(), 1);
    let conf = std::fs::read_to_string(d.fs.modprobe_d.join("iyakki-mac-keyboard-options.conf")).unwrap();
    assert!(conf.contains("options hid_apple fnmode=2"));
    assert!(!d.real && d.fs.modprobe_d.starts_with(&f.dir), "tests and desktop windows write only inside the data folder");
    // it shows in the installed list as a driver extension for the system
    let inst = f.store.installed(&f.svc, &iyakki_sec::policy::Role::Admin.template(), "root", &[]);
    let e = inst.iter().find(|e| e.name == "mac-keyboard-options").unwrap();
    assert_eq!((e.ext_kind.as_deref(), e.target.as_deref()), (Some("driver"), Some("system")));
    f.store.remove(&f.svc, &a, "extension", "mac-keyboard-options", &Scope::Shared).unwrap();
    assert!(d.installed().is_empty() && !d.fs.modprobe_d.join("iyakki-mac-keyboard-options.conf").exists());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn a_firmware_pack_from_a_usb_folder_is_reviewed_as_unsigned_and_only_an_administrator_installs_it() {
    let f = fx("drv-folder");
    let d = sandbox(&f);
    let dir = stick(&f, &[("BCM20702A1-05ac-8289.hcd", b"HCD-DATA"), ("readme.txt", b"x")]);
    let (text, taken) = d.folder_package("mac-bt-broadcom", &dir, NOW).unwrap();
    assert_eq!(taken.len(), 1);
    let a = admin();
    let r = f.store.review_sideload(&f.svc, &a, text.clone().into_bytes(), Scope::Shared, &[]);
    assert_eq!(r.verdict.level, Level::Unsigned);
    assert!(r.refusal.is_none() && r.needs_warning_ack(), "{}", r.summary());
    assert!(r.preview.iter().any(|l| l.contains("brcm/BCM20702A1-05ac-8289.hcd")) && r.permissions.iter().any(|p| p.words.contains("/lib/firmware")));
    assert!(r.warnings.iter().any(|w| w.contains("Nothing in this machine matches")) || d.machine().is_apple(), "{:?}", r.warnings);
    assert!(f.store.install(&f.svc, &a, &r, Confirm::default()).is_err(), "warnings must be accepted first");
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    assert_eq!(std::fs::read(d.fs.data.join("firmware/mac-bt-broadcom/files/brcm/BCM20702A1-05ac-8289.hcd")).unwrap(), b"HCD-DATA");
    assert!(iyakki_drivers::firmware_present(&d.fs, "brcm/BCM20702A1"));
    // not an administrator, or not for everyone: refused
    let u = user("alice");
    let r = f.store.review_sideload(&f.svc, &u, text.clone().into_bytes(), Scope::User("alice".into()), &[]);
    assert!(r.refusal.is_some());
    let r = f.store.review_sideload(&f.svc, &a, text.into_bytes(), Scope::User("root".into()), &[]);
    assert!(r.refusal.as_ref().unwrap().contains("whole machine"), "{}", r.summary());
    f.store.remove(&f.svc, &a, "extension", "mac-bt-broadcom", &Scope::Shared).unwrap();
    assert!(!iyakki_drivers::firmware_present(&d.fs, "brcm/BCM20702A1"));
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn kernel_modules_need_a_trusted_signature_and_the_exact_running_kernel() {
    let f = fx("drv-modules");
    let a = admin();
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string();
    let module = |kernel: &str, vermagic: &str| Pack { v: 1, modules: vec![pack::KoFile { name: "wl".into(), kernel: kernel.into(), data: b64::encode(&modinfo::synthetic_module("wl", vermagic, "MIXED/Proprietary")), sha256: String::new() }], ..Default::default() };
    // wrong kernel, even from the official key
    let p = module("6.12.100", "6.12.100 SMP mod_unload ");
    let r = f.store.review_sideload(&f.svc, &a, signed_pack(&f, "wl-pack", "1.0.0", &p, &f.root), Scope::Shared, &[]);
    assert_eq!(r.verdict.level, Level::Official);
    assert!(r.refusal.as_ref().is_some_and(|m| m.contains("6.12.100") && m.contains("exact kernel")), "{}", r.summary());
    // right kernel, but a signature nobody vouches for (Community)
    if !release.is_empty() {
        let stranger = new_signing_key().unwrap();
        let p = module(&release, &format!("{release} SMP mod_unload "));
        let r = f.store.review_sideload(&f.svc, &a, signed_pack(&f, "wl-pack", "1.0.0", &p, &stranger), Scope::Shared, &[]);
        assert_eq!(r.verdict.level, Level::Community);
        assert!(r.refusal.as_ref().is_some_and(|m| m.contains("Official, Verified or Trusted")), "{}", r.summary());
        // right kernel and an official signature: reviewed with a warning
        let r = f.store.review_sideload(&f.svc, &a, signed_pack(&f, "wl-pack", "1.0.0", &p, &f.root), Scope::Shared, &[]);
        assert!(r.refusal.is_none() && r.warnings.iter().any(|w| w.contains("kernel modules")), "{}", r.summary());
        assert!(r.permissions.iter().any(|p| p.words.contains("insmod")));
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// The Wi-Fi path end to end: a signed pack from the owner's own build, found on a USB stick, reviewed and installed only
/// when the owner's key is trusted, only for the running kernel, with the non-free warning.
#[test]
fn the_wifi_driver_pack_from_a_usb_stick_installs_only_for_a_trusted_key_and_this_kernel() {
    let f = fx("drv-wl");
    let a = admin();
    let d = sandbox(&f);
    let release = d.machine().release;
    if release.is_empty() {
        let _ = std::fs::remove_dir_all(&f.dir);
        return;
    }
    let owner = new_signing_key().unwrap();
    let wl = |kernel: &str, vermagic: &str, version: &str| {
        let ko = modinfo::synthetic_module_with("wl", vermagic, "MIXED/Proprietary", "cfg80211");
        let p = Pack {
            v: 1,
            matches: vec![iyakki_drivers::pack::Match { dmi_vendor: "Apple*".into(), pci: "14e4:43a0".into(), ..Default::default() }],
            modules: vec![pack::KoFile { name: "wl".into(), kernel: kernel.into(), data: b64::encode(&ko), sha256: pack::sha256_hex(&ko) }],
            modprobe: vec!["blacklist b43".into(), "blacklist bcma".into(), "blacklist ssb".into(), "blacklist brcmsmac".into()],
            unload: vec!["b43".into(), "bcma".into()],
            load: vec!["wl".into()],
            why: "Broadcom BCM4360 Wi-Fi: non-free driver, taints kernel".into(),
            ..Default::default()
        };
        signed_pack(&f, "mac-wifi-bcm4360", version, &p, &owner)
    };
    let good = format!("{release} SMP preempt mod_unload ");
    // the stick holds an older pack, a newer one and a pack for another entry: the newest of this entry is taken
    let dir = stick(&f, &[("old.iyp", &wl(&release, &good, "1.0.0")), ("mac-wifi-bcm4360-2.iyp", &wl(&release, &good, "1.0.1")), ("other.iyp", &signed_pack(&f, "mac-bt-other", "9.0.0", &fw_pack(), &owner)), ("readme.txt", b"x")]);
    let (text, taken) = d.folder_package("mac-wifi-bcm4360", &dir, NOW).unwrap();
    assert_eq!(taken.len(), 1);
    assert!(taken[0].contains("1.0.1") && taken[0].contains("signed"), "{taken:?}");
    // not yet trusted: Community, refused (a module is code that runs in the kernel)
    let r = f.store.review_sideload(&f.svc, &a, text.clone().into_bytes(), Scope::Shared, &[]);
    assert_eq!(r.verdict.level, Level::Community);
    assert!(r.refusal.as_ref().is_some_and(|m| m.contains("Official, Verified or Trusted")), "{}", r.summary());
    // the owner adds the public key under STORE, Settings: Trusted, reviewed with the non-free and kernel warnings
    f.store.trust_key(&f.svc, &a, &pkg::public_key_hex(&owner), "My Wi-Fi pack").unwrap();
    let r = f.store.review_sideload(&f.svc, &a, text.into_bytes(), Scope::Shared, &[]);
    assert_eq!(r.verdict.level, Level::Trusted, "{}", r.summary());
    assert!(r.refusal.is_none(), "{}", r.summary());
    assert!(r.warnings.iter().any(|w| w.contains("NON-FREE") && w.contains("tainted") && w.contains("MIXED/Proprietary")), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("kernel modules")));
    assert!(r.preview.iter().any(|l| l.contains("kernel module: wl.ko")) && r.preview.iter().any(|l| l.contains("blacklist bcma")), "{:?}", r.preview);
    assert!(r.permissions.iter().any(|p| p.words.contains("insmod")));
    f.store.install(&f.svc, &a, &r, Confirm::all()).unwrap();
    assert!(d.fs.data.join("firmware/mac-wifi-bcm4360/modules/wl.ko").exists());
    assert!(std::fs::read_to_string(d.fs.modprobe_d.join("iyakki-mac-wifi-bcm4360.conf")).unwrap().contains("blacklist ssb"));
    f.store.remove(&f.svc, &a, "extension", "mac-wifi-bcm4360", &Scope::Shared).unwrap();
    // a pack whose module was built for another kernel is refused whatever the pack claims, and so is a pack from a stranger
    let liar = wl(&release, "6.12.100 SMP preempt mod_unload ", "1.0.2");
    let r = f.store.review_sideload(&f.svc, &a, liar, Scope::Shared, &[]);
    assert!(r.refusal.as_ref().is_some_and(|m| m.contains("6.12.100") && m.contains("exact kernel")), "{}", r.summary());
    let r = f.store.review_sideload(&f.svc, &a, wl("6.12.100", "6.12.100 SMP preempt mod_unload ", "1.0.3"), Scope::Shared, &[]);
    assert!(r.refusal.is_some(), "{}", r.summary());
    // an empty stick tells the person what to do
    let empty = stick(&f, &[("notes.txt", b"x")]);
    let e = d.folder_package("mac-wifi-bcm4360", &empty, NOW).unwrap_err();
    assert!(e.contains("no signed driver pack") && e.contains("build-wl-pack.sh"), "{e}");
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// The real Broadcom module is about 7.5 MB: the pack and the package around it must have room for it.
#[test]
fn a_module_of_the_size_of_the_real_wifi_driver_fits_in_a_pack_and_a_package() {
    let mut ko = modinfo::synthetic_module("wl", "6.12.112 SMP preempt mod_unload ", "MIXED/Proprietary");
    ko.extend(std::iter::repeat_n(0x5au8, 7_600_000));
    let p = Pack { v: 1, modules: vec![pack::KoFile { name: "wl".into(), kernel: "6.12.112".into(), data: b64::encode(&ko), sha256: pack::sha256_hex(&ko) }], ..Default::default() };
    p.validate().unwrap();
    let key = new_signing_key().unwrap();
    let text = signed_pack(&fx("drv-big"), "mac-wifi-bcm4360", "1.0.0", &p, &key);
    assert!(text.len() > 10_000_000 && text.len() < pkg::MAX_EXTENSION_PACKAGE_BYTES, "{}", text.len());
    assert!(pkg::parse(std::str::from_utf8(&text).unwrap()).is_ok());
    let mut huge = p.clone();
    huge.modules[0].data = b64::encode(&vec![1u8; pack::MAX_MODULE_BYTES + 1]);
    assert!(huge.validate().is_err());
}

#[test]
fn a_bad_pack_payload_is_refused_in_the_review_before_anything_is_written() {
    let f = fx("drv-bad");
    let a = admin();
    let mut p = fw_pack();
    p.modprobe = vec!["install btusb /bin/sh".into()];
    let r = f.store.review_sideload(&f.svc, &a, signed_pack(&f, "evil", "1.0.0", &p, &f.root), Scope::Shared, &[]);
    assert!(r.refusal.as_ref().is_some_and(|m| m.contains("nothing that runs a command")), "{}", r.summary());
    let mut p = fw_pack();
    p.files[0].path = "../../etc/passwd".into();
    let r = f.store.review_sideload(&f.svc, &a, signed_pack(&f, "evil", "1.0.0", &p, &f.root), Scope::Shared, &[]);
    assert!(r.refusal.is_some());
    assert!(sandbox(&f).installed().is_empty());
    let _ = std::fs::remove_dir_all(&f.dir);
}

#[test]
fn the_camera_firmware_needs_the_licence_notice_accepted_and_a_driver_file_it_knows() {
    let f = fx("drv-cam");
    let d = sandbox(&f);
    let e = d.facetime_package(b"anything", false, NOW).unwrap_err();
    assert!(e.contains("Apple") && e.contains("accept"), "{e}");
    let e = d.facetime_package(b"not the apple driver", true, NOW).unwrap_err();
    assert!(e.contains("not a build this tool knows") && e.contains("Nothing was extracted"), "{e}");
    let _ = std::fs::remove_dir_all(&f.dir);
}

fn fake_mini(tag: &str) -> Drivers {
    let root = std::env::temp_dir().join(format!("iyakki-drv-mini-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (rel, c) in [("sys/class/dmi/id/sys_vendor", "Apple Inc.\n"), ("sys/class/dmi/id/product_name", "Macmini7,1\n"), ("proc/sys/kernel/osrelease", "6.12.112\n"), ("proc/modules", "btusb 1 0 - Live 0x0\n"), ("sys/bus/pci/devices/0000:03:00.0/vendor", "0x14e4\n"), ("sys/bus/pci/devices/0000:03:00.0/device", "0x43a0\n"), ("sys/bus/usb/devices/1-4/idVendor", "05ac\n"), ("sys/bus/usb/devices/1-4/idProduct", "8289\n"), ("sys/bus/usb/devices/1-4/product", "Bluetooth USB Host Controller\n")] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, c).unwrap();
    }
    Drivers::for_data(&root.join("data")).with_roots(Roots { sys: root.join("sys"), proc: root.join("proc") })
}

#[test]
fn driver_status_tells_a_mac_mini_what_is_missing_and_a_generic_pc_that_nothing_applies() {
    let d = fake_mini("status");
    let t = status_text(&d);
    assert!(t.contains("Apple Inc. Macmini7,1") && t.contains("kernel 6.12.112"), "{t}");
    assert!(t.contains("mac-bt-broadcom") && t.contains("firmware missing") && t.contains("works after installing the pack"), "{t}");
    assert!(t.contains("mac-wifi-bcm4360") && t.contains("works with a locally built non-free driver pack") && t.contains("build the pack on a Linux PC") && t.contains("USB Wi-Fi adapter") && !t.contains("not supported"), "{t}");
    assert!(t.contains("mac-audio-cirrus") && t.contains("needs the Mac image variant"), "{t}");
    assert!(!t.contains("mac-facetimehd") && !t.contains("mac-t2"), "no camera and no T2 on this box:\n{t}");
    let empty = std::env::temp_dir().join(format!("iyakki-drv-pc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(empty.join("sys")).unwrap();
    std::fs::create_dir_all(empty.join("proc")).unwrap();
    let pc = Drivers::for_data(&empty.join("data")).with_roots(Roots { sys: empty.join("sys"), proc: empty.join("proc") });
    assert!(status_text(&pc).contains("No driver packs apply"), "{}", status_text(&pc));
}

#[test]
fn the_assistant_tools_exist_with_the_store_status_is_safe_and_install_is_hard_ask() {
    let p = iyakki_sec::policy::Role::Admin.template();
    let d = defs(&p);
    assert_eq!(d.iter().map(|d| d.spec.name.as_str()).collect::<Vec<_>>(), vec!["driver_status", "driver_install"]);
    assert_eq!((d[0].risk, d[1].risk), (crate::agent::Risk::Safe, crate::agent::Risk::Ask));
    let mut none = p.clone();
    none.apps.remove(&App::Store);
    assert!(defs(&none).is_empty());
    assert!(crate::agent::hard_ask("driver_install", &json!({"pack": "mac-bt-broadcom"})).is_some(), "Auto and Autopilot never install drivers");
    assert!(crate::agent::hard_ask("driver_status", &json!({})).is_none());
    assert!(is_driver_tool("driver_install") && !is_driver_tool("store_install"));
}

#[test]
fn driver_install_goes_through_the_same_review_installs_from_a_folder_and_refuses_what_it_cannot_do() {
    let f = fx("drv-tool");
    let p = iyakki_sec::policy::Role::Admin.template();
    let dir = stick(&f, &[("BCM20702A1-0a5c-21e6.hcd", b"HCD-DATA")]);
    let call = |args: serde_json::Value| super::call("driver_install", &args, &f.svc, &p, "root");
    assert!(call(json!({"pack": "nope"})).unwrap_err().contains("not a driver pack"));
    assert!(call(json!({"pack": "mac-t2"})).unwrap_err().contains("cannot be installed"));
    assert!(call(json!({"pack": "mac-bt-broadcom"})).unwrap_err().contains("folder or USB"));
    assert!(call(json!({"pack": "mac-facetimehd", "source": "/nonexistent"})).unwrap_err().contains("Apple"), "the licence notice comes first");
    let e = call(json!({"pack": "mac-facetimehd", "source": "/nonexistent", "accept_licence": true})).unwrap_err();
    assert!(e.contains("/nonexistent"), "{e}");
    let ok = call(json!({"pack": "mac-bt-broadcom", "source": dir.to_string_lossy()})).unwrap();
    assert!(ok.contains("Installed"), "{ok}");
    assert!(iyakki_drivers::firmware_present(&sandbox(&f).fs, "brcm/BCM20702A1"));
    // a user without the right is refused by the review, not by luck
    let up = iyakki_sec::policy::Role::User.template();
    let e = super::call("driver_install", &json!({"pack": "mac-bt-broadcom", "source": dir.to_string_lossy()}), &f.svc, &up, "alice").unwrap_err();
    assert!(e.contains("Not installed"), "{e}");
    let st = super::call("driver_status", &json!({}), &f.svc, &p, "root").unwrap();
    assert!(st.contains("Machine:") && st.contains("Installed packs: mac-bt-broadcom"), "{st}");
    let _ = (Store::open(&f.dir), builtin::VERSION);
    let _ = std::fs::remove_dir_all(&f.dir);
}

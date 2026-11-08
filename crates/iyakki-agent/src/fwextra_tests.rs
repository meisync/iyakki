use super::*;
use crate::archive::tests::{gzip, make_tar, tmp};

// ------------------------------------------------------------------------------------------------ fixtures

const FILES: &[(&str, &str, &str)] = &[
    ("intel/iwlwifi/iwlwifi-cc-a0-77.ucode", "ucode 77 bytes", "LICENCE.iwlwifi_firmware"),
    ("intel/iwlwifi/iwlwifi-cc-a0-72.ucode", "ucode 72 bytes", "LICENCE.iwlwifi_firmware"),
    ("intel/ibt-0041-0041.sfi", "bluetooth patch", "LICENCE.ibt_firmware"),
    ("i915/tgl_guc_70.bin", "guc blob", "LICENSE.i915"),
    ("rtw89/rtw8852a_fw.bin", "realtek wifi", "LICENCE.rtlwifi_firmware.txt"),
    ("mediatek/mt7921/WIFI_RAM_CODE_MT7961_1.bin", "mediatek wifi", "LICENCE.mediatek"),
    ("nvidia/ga102/gsp/booter_load-570.144.bin", "nvidia booter", "LICENCE.nvidia"),
    ("tigon/tg3.bin", "tigon", "LICENCE.tigon"),
];

const DOCS: &[(&str, &str)] = &[
    ("LICENSES/LICENCE.iwlwifi_firmware", "Intel licence text"),
    ("LICENSES/LICENCE.ibt_firmware", "Intel BT licence text"),
    ("LICENSES/LICENSE.i915", "i915 licence"),
    ("LICENSES/LICENCE.rtlwifi_firmware.txt", "realtek licence"),
    ("LICENSES/LICENCE.mediatek", "mediatek licence"),
    ("LICENSES/LICENCE.nvidia", "nvidia licence"),
    ("LICENSES/LICENCE.tigon", "tigon licence"),
];

const WHENCE: &str = "WHENCE header\n\n--------------------------------------------------------------------------\n\nDriver: iwlwifi - Intel Wi-Fi\n\nFile: intel/iwlwifi/iwlwifi-cc-a0-77.ucode\nLink: iwlwifi-cc-a0-77.ucode -> intel/iwlwifi/iwlwifi-cc-a0-77.ucode\n\nLicence: Redistributable. See LICENCE.iwlwifi_firmware for details\n\n--------------------------------------------------------------------------\n\nDriver: tg3 - Broadcom\n\nFile: tigon/tg3.bin\n\nLicence: Redistributable. See LICENCE.tigon for details\n";

fn sha(b: &[u8]) -> String {
    sha256_hex(b)
}

fn fake_manifest() -> Manifest {
    let mut t = format!("tag {TAG}\ncommit {COMMIT}\nbase https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain\nsnapshot 100 {} https://cdn.kernel.org/pub/linux/kernel/firmware/linux-firmware-{TAG}.tar.gz\n", "ab".repeat(32));
    t.push_str(&format!("doc {} {} WHENCE\n", WHENCE.len(), sha(WHENCE.as_bytes())));
    for (p, b) in DOCS {
        t.push_str(&format!("doc {} {} {p}\n", b.len(), sha(b.as_bytes())));
    }
    for (p, b, tok) in FILES {
        t.push_str(&format!("file {} {} {tok} {p}\n", b.len(), sha(b.as_bytes())));
    }
    t.push_str("link iwlwifi-cc-a0-77.ucode intel/iwlwifi/iwlwifi-cc-a0-77.ucode\nlink iwlwifi-cc-a0-72.ucode intel/iwlwifi/iwlwifi-cc-a0-72.ucode\nlink brcm/alias.bin ../tigon/tg3.bin\n");
    parse_manifest(&t).unwrap()
}

fn bytes_of(path: &str) -> Vec<u8> {
    if path == "WHENCE" {
        return WHENCE.as_bytes().to_vec();
    }
    FILES.iter().find(|f| f.0 == path).map(|f| f.1.as_bytes().to_vec()).or_else(|| DOCS.iter().find(|d| d.0 == path).map(|d| d.1.as_bytes().to_vec())).unwrap_or_else(|| panic!("no fixture {path}"))
}

const LOG: &str = "[    7.100000] iwlwifi 0000:00:14.3: Direct firmware load for iwlwifi-cc-a0-77.ucode failed with error -2\n\
[    7.100100] iwlwifi 0000:00:14.3: Direct firmware load for iwlwifi-cc-a0-76.ucode failed with error -2\n\
[    7.100200] iwlwifi 0000:00:14.3: no suitable firmware found!\n\
[    7.200000] i915 0000:00:02.0: [drm] *ERROR* GuC firmware i915/tgl_guc_70.bin: fetch failed with error -2\n\
[    7.300000] r8169 0000:03:00.0: Direct firmware load for rtl_nic/not-in-release.fw failed with error -2\n\
[    7.400000] Bluetooth: hci0: Direct firmware load for intel/ibt-0041-0041.sfi failed with error -2\n\
[    7.500000] usb 1-1: new high-speed USB device number 2 using xhci_hcd\n\
[    7.600000] ata1: SATA link up 6.0 Gbps\n";

// ------------------------------------------------------------------------------------------------ the pinned manifest

#[test]
fn the_pinned_manifest_matches_the_constants_and_is_sane() {
    let m = manifest();
    assert_eq!(m.tag, TAG);
    assert_eq!(m.commit, COMMIT);
    assert_eq!(m.snapshot_sha256, SNAPSHOT_SHA256, "the snapshot hash in the code is the one in the manifest");
    assert_eq!(m.snapshot_size, 1_021_468_302);
    assert!(m.snapshot_url.ends_with(&format!("linux-firmware-{TAG}.tar.gz")) && m.snapshot_url.starts_with("https://cdn.kernel.org/"));
    assert!(m.base.starts_with("https://git.kernel.org/"));
    assert!(m.files.len() > 4000 && m.docs.contains_key("WHENCE"));
    // the real release has what the examples in the docs say
    let r = m.resolve("iwlwifi-cc-a0-77.ucode").expect("a root-level link resolves");
    assert_eq!(r.path, "intel/iwlwifi/iwlwifi-cc-a0-77.ucode");
    assert!(m.files.values().all(|f| !f.tokens.is_empty() && f.size > 0));
    // every licence text a file names exists as a document (or is one of the few the release does not hold)
    let missing: BTreeSet<&String> = m.files.values().flat_map(|f| &f.tokens).filter(|t| !m.docs.contains_key(&format!("LICENSES/{t}"))).collect();
    assert!(missing.len() < 12, "licence texts missing from the manifest: {missing:?}");
    assert!(m.docs.contains_key("LICENSES/LICENCE.nvidia") && m.docs.contains_key("LICENSES/LICENCE.iwlwifi_firmware"));
}

#[test]
fn a_bad_manifest_is_refused_not_half_used() {
    assert!(parse_manifest("tag 1\n").is_err());
    let good = format!("tag {TAG}\ncommit {COMMIT}\nbase https://x.example/plain\nsnapshot 1 {} https://x.example/a.tar.gz\ndoc 1 {} WHENCE\nfile 1 {} T a/b.bin\n", "a".repeat(64), "b".repeat(64), "c".repeat(64));
    assert!(parse_manifest(&good).is_ok());
    assert!(parse_manifest(&good.replace(&"c".repeat(64), "zz")).is_err(), "a short hash");
    assert!(parse_manifest(&format!("{good}garbage line here\n")).is_err());
    // a path with a dot component or an absolute path is left out, never kept
    let m = parse_manifest(&format!("{good}file 1 {} T ../etc/passwd\nfile 1 {} T /abs\n", "c".repeat(64), "c".repeat(64))).unwrap();
    assert_eq!(m.files.len(), 1);
}

#[test]
fn names_resolve_through_links_and_never_leave_the_release() {
    let m = fake_manifest();
    assert_eq!(m.resolve("iwlwifi-cc-a0-77.ucode").unwrap().path, "intel/iwlwifi/iwlwifi-cc-a0-77.ucode");
    assert_eq!(m.resolve("brcm/alias.bin").unwrap().path, "tigon/tg3.bin", "a relative link with ..");
    assert!(m.resolve("../etc/passwd").is_none() && m.resolve("/etc/passwd").is_none() && m.resolve("nope.bin").is_none());
}

// ------------------------------------------------------------------------------------------------ the kernel log

#[test]
fn the_kernel_log_gives_the_missing_files_with_driver_and_device() {
    let m = fake_manifest();
    let reqs = parse_log(LOG, &m);
    let names: Vec<&str> = reqs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["iwlwifi-cc-a0-77.ucode", "iwlwifi-cc-a0-76.ucode", "i915/tgl_guc_70.bin", "rtl_nic/not-in-release.fw", "intel/ibt-0041-0041.sfi"]);
    assert_eq!((reqs[0].driver.as_str(), reqs[0].device.as_str()), ("iwlwifi", "0000:00:14.3"));
    assert_eq!(reqs[4].driver, "Bluetooth");
}

#[test]
fn other_log_shapes_are_understood_and_noise_is_not() {
    let m = fake_manifest();
    let t = "<6>kernel: tg3 0000:02:00.0: firmware: failed to load tigon/tg3.bin (-2)\n\
             May 01 host kernel: Falling back to sysfs fallback for: rtw89/rtw8852a_fw.bin\n\
             [ 1.0] mt7921e 0000:04:00.0: request_firmware failed for mediatek/mt7921/WIFI_RAM_CODE_MT7961_1.bin, error -2 firmware missing\n\
             [ 1.1] firmware: direct-loading firmware regulatory.db\n\
             [ 1.2] Direct firmware load for ../../etc/shadow failed with error -2\n";
    let names: Vec<String> = parse_log(t, &m).into_iter().map(|r| r.name).collect();
    assert_eq!(names, ["tigon/tg3.bin", "rtw89/rtw8852a_fw.bin", "mediatek/mt7921/WIFI_RAM_CODE_MT7961_1.bin"], "a direct-loading success and a traversal name are not requests");
    assert!(parse_log("", &m).is_empty());
}

#[test]
fn versions_of_one_family_need_only_the_first_the_release_has() {
    let m = fake_manifest();
    let reqs = parse_log(LOG, &m);
    let (ns, alt) = needs(&reqs, &m);
    let shown: Vec<&str> = ns.iter().map(|n| n.shown.as_str()).collect();
    assert!(shown.contains(&"iwlwifi-cc-a0-*.ucode"), "{shown:?}");
    assert_eq!(ns.iter().filter(|n| n.shown.starts_with("iwlwifi")).count(), 1);
    assert_eq!(alt, ["iwlwifi-cc-a0-76.ucode"]);
    // when the newest version asked for is newer than the release, the first one the release has is used
    let t = "iwlwifi 0000:00:14.3: Direct firmware load for iwlwifi-cc-a0-99.ucode failed with error -2\niwlwifi 0000:00:14.3: Direct firmware load for iwlwifi-cc-a0-72.ucode failed with error -2\n";
    let (ns, alt) = needs(&parse_log(t, &m), &m);
    assert_eq!(ns.len(), 1);
    assert_eq!(ns[0].resolved.as_ref().unwrap().path, "intel/iwlwifi/iwlwifi-cc-a0-72.ucode");
    assert_eq!(alt, ["iwlwifi-cc-a0-99.ucode"]);
    assert_eq!(family_key("iwlwifi-9000-pu-b0-jf-b0-46.ucode").as_deref(), Some("iwlwifi-9000-pu-b0-jf-b0-*.ucode"));
    assert_eq!(family_key("intel/ibt-0041-0041.sfi"), None);
}

// ------------------------------------------------------------------------------------------------ PCI and USB ids

fn fake_sys(tag: &str) -> PathBuf {
    let sys = tmp(tag);
    let wifi = sys.join("bus/pci/devices/0000:00:14.3");
    std::fs::create_dir_all(&wifi).unwrap();
    for (f, v) in [("vendor", "0x8086"), ("device", "0x2723"), ("class", "0x028000")] {
        std::fs::write(wifi.join(f), format!("{v}\n")).unwrap();
    }
    let gpu = sys.join("bus/pci/devices/0000:00:02.0");
    std::fs::create_dir_all(&gpu).unwrap();
    for (f, v) in [("vendor", "0x8086"), ("device", "0x9a49"), ("class", "0x030000")] {
        std::fs::write(gpu.join(f), format!("{v}\n")).unwrap();
    }
    std::os::unix::fs::symlink("../../../../bus/pci/drivers/i915", gpu.join("driver")).unwrap();
    let bt = sys.join("bus/usb/devices/1-1");
    std::fs::create_dir_all(&bt).unwrap();
    for (f, v) in [("idVendor", "8087"), ("idProduct", "0033"), ("bDeviceClass", "e0")] {
        std::fs::write(bt.join(f), format!("{v}\n")).unwrap();
    }
    std::fs::create_dir_all(sys.join("bus/usb/devices/1-1:1.0")).unwrap();
    sys
}

#[test]
fn devices_are_read_from_sysfs_and_named_for_the_person() {
    let sys = fake_sys("fwx-sys");
    let devs = read_devs(&sys);
    assert_eq!(devs.len(), 3);
    let wifi = devs.iter().find(|d| d.id == "0000:00:14.3").unwrap();
    assert_eq!(dev_phrase(wifi), "your Wi-Fi card Intel Wi-Fi 6 AX200 (8086:2723)");
    // the HP Pavilion 16-af0 (2026-11-07): Realtek RTL8852BE Wi-Fi on PCI and its Bluetooth half on USB
    assert_eq!(known_name(0x10ec, 0xb852), Some("RTL8852BE Wi-Fi"));
    assert_eq!(known_name(0x0bda, 0xb85c), Some("RTL8852BE Bluetooth"));
    let gpu = devs.iter().find(|d| d.driver == "i915").unwrap();
    assert_eq!(class_label(gpu), "graphics card");
    assert_eq!(class_label(devs.iter().find(|d| d.bus == "usb").unwrap()), "Bluetooth adapter");
    // the owner of a request: by the address in the log, else by the driver name
    let r = Request { name: "x".into(), driver: "iwlwifi".into(), device: "0000:00:14.3".into() };
    assert_eq!(owner(&r, &devs).unwrap().id, "0000:00:14.3");
    let r = Request { name: "x".into(), driver: "i915".into(), device: String::new() };
    assert_eq!(owner(&r, &devs).unwrap().id, "0000:00:02.0");
    let _ = std::fs::remove_dir_all(&sys);
}

// ------------------------------------------------------------------------------------------------ the plan and the licence list

#[test]
fn the_plan_groups_by_vendor_says_why_and_lists_the_licences_before_the_click() {
    let m = fake_manifest();
    let sys = fake_sys("fwx-plan");
    let mut text = LOG.to_string();
    text.push_str("[ 9.0] nouveau 0000:01:00.0: Direct firmware load for nvidia/ga102/gsp/booter_load-570.144.bin failed with error -2\n[ 9.1] mt7921e 0000:04:00.0: Direct firmware load for mediatek/mt7921/WIFI_RAM_CODE_MT7961_1.bin failed with error -2\n[ 9.2] tg3 0000:02:00.0: Direct firmware load for tigon/tg3.bin failed with error -2\n");
    let plan = build_plan(&parse_log(&text, &m), &read_devs(&sys), &m);
    let titles: Vec<&str> = plan.groups.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, ["Broadcom Ethernet", "Intel Bluetooth", "Intel Wi-Fi", "Intel graphics", "MediaTek Wi-Fi and Bluetooth", "NVIDIA graphics"], "NVIDIA last, shown separately");
    let g = |t: &str| plan.groups.iter().find(|g| g.title == t).unwrap();
    assert_eq!(g("Intel Wi-Fi").class(), Class::Free);
    assert_eq!(g("Intel Wi-Fi").class().label(), "Free to redistribute with notice");
    assert_eq!(g("MediaTek Wi-Fi and Bluetooth").class(), Class::VendorOnly);
    assert_eq!(g("MediaTek Wi-Fi and Bluetooth").class().label(), "Restricted to the vendor's hardware");
    assert_eq!(g("Broadcom Ethernet").class(), Class::OldWording);
    assert!(g("NVIDIA graphics").separate && g("NVIDIA graphics").needs.contains("Turing"));
    assert_eq!(plan.unknown, ["rtl_nic/not-in-release.fw"]);
    let lines = plan.lines().join("\n");
    assert!(lines.contains("your Wi-Fi card Intel Wi-Fi 6 AX200 (8086:2723) needs iwlwifi-cc-a0-*.ucode"), "{lines}");
    assert!(lines.contains("licence LICENCE.iwlwifi_firmware") && lines.contains("licence LICENCE.nvidia") && lines.contains("only with processors designed by NVIDIA"));
    assert!(lines.contains("not in linux-firmware 20261019: rtl_nic/not-in-release.fw"));
    // the separate group is left out unless the person includes it
    let without = plan.select(&[]);
    assert!(!without.files.iter().any(|f| f.path.starts_with("nvidia/")));
    let with = plan.select(&["nvidia".to_string()]);
    assert!(with.files.iter().any(|f| f.path.starts_with("nvidia/")));
    assert_ne!(without.digest(&m), with.digest(&m), "another list asks again");
    // licence texts and WHENCE ride along
    let docs: Vec<String> = with.docs(&m).keys().cloned().collect();
    assert!(docs.contains(&"WHENCE".to_string()) && docs.contains(&"LICENSES/LICENCE.nvidia".to_string()) && docs.contains(&"LICENSES/LICENCE.iwlwifi_firmware".to_string()));
    let _ = std::fs::remove_dir_all(&sys);
}

#[test]
fn the_whence_excerpt_has_only_the_blocks_of_the_installed_files() {
    let x = whence_excerpt(WHENCE, &["iwlwifi-cc-a0-77.ucode".to_string(), "intel/iwlwifi/iwlwifi-cc-a0-77.ucode".to_string()]);
    assert!(x.contains("Driver: iwlwifi") && x.contains("LICENCE.iwlwifi_firmware") && !x.contains("tg3"), "{x}");
}

// ------------------------------------------------------------------------------------------------ the job with a fake world

struct Fake {
    log: String,
    devs: Vec<Dev>,
    free: Option<u64>,
    in_memory: bool,
    /// served by the "network", by path
    serve: BTreeMap<String, Vec<u8>>,
    fetched: Vec<String>,
    installed: Vec<pack::Pack>,
    report: Vec<String>,
    mods: Vec<(String, ModState)>,
    audit: Vec<String>,
}

impl Fake {
    fn new() -> Fake {
        let mut serve: BTreeMap<String, Vec<u8>> = FILES.iter().map(|f| (f.0.to_string(), f.1.as_bytes().to_vec())).collect();
        for d in DOCS {
            serve.insert(d.0.to_string(), d.1.as_bytes().to_vec());
        }
        serve.insert("WHENCE".into(), WHENCE.as_bytes().to_vec());
        Fake { log: LOG.into(), devs: vec![], free: Some(1 << 30), in_memory: false, serve, fetched: vec![], installed: vec![], report: vec!["driver iwlwifi reloaded".into()], mods: vec![("iwlwifi".into(), ModState::Loaded), ("Bluetooth".into(), ModState::Absent)], audit: vec![] }
    }
}

impl Env for Fake {
    fn say(&mut self, _l: &str) {}
    fn audit(&mut self, l: &str) {
        self.audit.push(l.to_string());
    }
    fn free_bytes(&self, _d: &Path) -> Option<u64> {
        self.free
    }
    fn data_in_memory(&self) -> bool {
        self.in_memory
    }
    fn kernel_log(&self) -> String {
        self.log.clone()
    }
    fn devs(&self) -> Vec<Dev> {
        self.devs.clone()
    }
    fn fetch(&mut self, url: &str, _want: u64, dest: &Path) -> Result<(), String> {
        self.fetched.push(url.to_string());
        let path = url.split("/plain/").nth(1).unwrap().split('?').next().unwrap().to_string();
        assert!(url.ends_with(&format!("?id={COMMIT}")), "the pinned commit is in every address: {url}");
        let b = self.serve.get(&path).ok_or(format!("404 {path}"))?;
        std::fs::write(dest, b).map_err(|e| e.to_string())
    }
    fn install(&mut self, _v: &str, _s: &str, p: &pack::Pack) -> Result<apply::Report, String> {
        self.installed.push(p.clone());
        Ok(apply::Report { lines: self.report.clone(), cmdline: vec![] })
    }
    fn module_state(&self, n: &str) -> ModState {
        self.mods.iter().find(|m| m.0 == n).map_or(ModState::Absent, |m| m.1)
    }
    fn now(&self) -> i64 {
        1_700_000_000
    }
}

fn selection(env: &Fake, m: &Manifest, data: &Path, with: &[String]) -> Selection {
    plan_in(m, env, data).select(with)
}

#[test]
fn nothing_is_fetched_before_the_licence_list_is_accepted() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-noaccept"), Fake::new());
    let e = run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap_err();
    assert!(e.contains("not accepted"), "{e}");
    assert!(env.fetched.is_empty() && env.installed.is_empty());
    assert!(read_outcome(&data).unwrap().headline.starts_with("Not done"));
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_changed_list_asks_again() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-changed"), Fake::new());
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    env.log.push_str("[ 9.0] tg3 0000:02:00.0: Direct firmware load for tigon/tg3.bin failed with error -2\n");
    let e = run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap_err();
    assert!(e.contains("not accepted") || e.contains("changed"), "{e}");
    assert!(env.fetched.is_empty());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn download_checks_every_file_installs_the_needed_ones_and_reloads_safe_drivers_only() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-ok"), Fake::new());
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    let o = run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap();
    assert!(o.installed && o.source == "download");
    // only the needed files and their licence texts, never the rest of the release
    assert!(env.fetched.iter().all(|u| u.contains("/plain/") && u.starts_with("https://git.kernel.org/")));
    assert!(!env.fetched.iter().any(|u| u.contains("tigon") || u.contains("mediatek") || u.contains("nvidia") || u.contains("iwlwifi-cc-a0-72")), "{:?}", env.fetched);
    let p = &env.installed[0];
    let paths: Vec<&str> = p.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"intel/iwlwifi/iwlwifi-cc-a0-77.ucode") && paths.contains(&"i915/tgl_guc_70.bin") && paths.contains(&"intel/ibt-0041-0041.sfi"));
    assert!(paths.contains(&"licences/fw-hardware/LICENCE.iwlwifi_firmware") && paths.contains(&"licences/fw-hardware/WHENCE.excerpt"), "{paths:?}");
    assert!(p.files.iter().all(|f| f.sha256.len() == 64));
    // the graphics driver is never reloaded; a restart is asked for in words
    assert_eq!(p.reload, ["iwlwifi"]);
    assert!(o.reboot_needed && o.lines.iter().any(|l| l.contains("i915 may drive the screen")), "{:?}", o.lines);
    assert!(env.audit.iter().any(|a| a.starts_with("firmware.start")) && env.audit.iter().any(|a| a.starts_with("firmware.installed")));
    assert!(env.audit.iter().all(|a| !a.contains("/home") && !a.to_lowercase().contains("password")), "no secrets, no paths");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_wifi_only_list_needs_no_restart_when_the_driver_reloads() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-norestart"), Fake::new());
    env.log = LOG.lines().filter(|l| l.contains("iwlwifi")).collect::<Vec<_>>().join("\n");
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    let o = run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap();
    assert!(!o.reboot_needed && o.headline.contains("loaded again"), "{o:?}");
    // a built-in driver can only read it at start
    let (data2, mut env2) = (tmp("fwx-builtin"), Fake::new());
    env2.log = env.log.clone();
    env2.mods = vec![("iwlwifi".into(), ModState::Builtin)];
    accept_in(&m, &data2, &selection(&env2, &m, &data2, &[]), "admin", 5).unwrap();
    assert!(run_with(&mut env2, &m, &data2, &Source::Download, &Opts::default()).unwrap().reboot_needed);
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&data2);
}

#[test]
fn a_hash_mismatch_installs_nothing_and_deletes_the_bad_file() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-hash"), Fake::new());
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    env.serve.insert("i915/tgl_guc_70.bin".into(), b"guc blob".iter().map(|b| b ^ 1).collect());
    let e = run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap_err();
    assert!(e.contains("sha256") && e.contains("Nothing was installed"), "{e}");
    assert!(env.installed.is_empty());
    assert!(!data.join("fwextra/download/i915/tgl_guc_70.bin").exists());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_size_that_differs_is_refused_too() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-size"), Fake::new());
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    env.serve.insert("intel/ibt-0041-0041.sfi".into(), b"bluetooth patch plus more".to_vec());
    assert!(run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap_err().contains("bytes"));
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn too_little_free_space_and_a_memory_volume_stop_the_job_before_any_download() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-space"), Fake::new());
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    env.free = Some(10 << 20);
    let e = run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap_err();
    assert!(e.contains("MB free"), "{e}");
    env.free = Some(1 << 30);
    env.in_memory = true;
    assert!(run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap_err().contains("in memory"));
    assert!(env.fetched.is_empty());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn nothing_missing_means_nothing_to_do() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-none"), Fake::new());
    env.log = "[ 1.0] usb 1-1: new device\n".into();
    let e = run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap_err();
    assert!(e.contains("no missing firmware"), "{e}");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn the_separate_group_is_installed_only_when_included_and_accepted() {
    let (m, data, mut env) = (fake_manifest(), tmp("fwx-nv"), Fake::new());
    env.log = "[ 9.0] nouveau 0000:01:00.0: Direct firmware load for nvidia/ga102/gsp/booter_load-570.144.bin failed with error -2\n".into();
    let e = run_with(&mut env, &m, &data, &Source::Download, &Opts::default()).unwrap_err();
    assert!(e.contains("no missing firmware"), "left out unless included: {e}");
    let with = Opts { with: vec!["nvidia".into()] };
    assert!(run_with(&mut env, &m, &data, &Source::Download, &with).unwrap_err().contains("not accepted"));
    accept_in(&m, &data, &selection(&env, &m, &data, &with.with), "admin", 5).unwrap();
    let o = run_with(&mut env, &m, &data, &Source::Download, &with).unwrap();
    assert!(o.installed && o.reboot_needed, "graphics firmware needs a restart");
    let _ = std::fs::remove_dir_all(&data);
}

// ------------------------------------------------------------------------------------------------ the folder, stick and staging sources

fn stick_with(root: &Path, files: &[&str]) {
    for p in files {
        let dest = root.join("firmware-extra").join(p);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(dest, bytes_of(p)).unwrap();
    }
}

const NEEDED: &[&str] = &["WHENCE", "LICENSES/LICENCE.iwlwifi_firmware", "LICENSES/LICENCE.ibt_firmware", "LICENSES/LICENSE.i915", "intel/iwlwifi/iwlwifi-cc-a0-77.ucode", "i915/tgl_guc_70.bin", "intel/ibt-0041-0041.sfi"];

#[test]
fn a_stick_with_good_files_installs_without_any_network() {
    let (m, data, stick, mut env) = (fake_manifest(), tmp("fwx-stick-d"), tmp("fwx-stick-s"), Fake::new());
    stick_with(&stick, NEEDED);
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    let o = run_with(&mut env, &m, &data, &Source::Folder(stick.clone()), &Opts::default()).unwrap();
    assert!(o.installed && o.source == "folder" && env.fetched.is_empty());
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&stick);
}

#[test]
fn a_stick_with_a_wrong_file_is_refused_whole() {
    let (m, data, stick, mut env) = (fake_manifest(), tmp("fwx-bad-d"), tmp("fwx-bad-s"), Fake::new());
    stick_with(&stick, NEEDED);
    std::fs::write(stick.join("firmware-extra/i915/tgl_guc_70.bin"), b"another release").unwrap();
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    let e = run_with(&mut env, &m, &data, &Source::Folder(stick.clone()), &Opts::default()).unwrap_err();
    assert!(e.contains("Nothing was installed") && env.installed.is_empty(), "{e}");
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&stick);
}

#[test]
fn a_folder_missing_a_needed_file_names_it() {
    let (m, data, stick, mut env) = (fake_manifest(), tmp("fwx-miss-d"), tmp("fwx-miss-s"), Fake::new());
    stick_with(&stick, &NEEDED[..5]);
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    let e = run_with(&mut env, &m, &data, &Source::Folder(stick.clone()), &Opts::default()).unwrap_err();
    assert!(e.contains("not in") && e.contains("fetch.sh"), "{e}");
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&stick);
}

#[test]
fn links_and_odd_names_in_a_staging_folder_are_never_followed() {
    let (m, data, stage, mut env) = (fake_manifest(), tmp("fwx-link-d"), tmp("fwx-link-s"), Fake::new());
    stick_with(&stage, &NEEDED[..4]);
    // a push that tries to plant a link to a file outside, in place of a needed file
    let outside = stage.join("secret.txt");
    std::fs::write(&outside, b"intel/ibt-0041-0041.sfi stand-in").unwrap();
    std::fs::create_dir_all(stage.join("firmware-extra/intel")).unwrap();
    std::os::unix::fs::symlink(&outside, stage.join("firmware-extra/intel/ibt-0041-0041.sfi")).unwrap();
    std::fs::write(stage.join("firmware-extra/..%2f..%2fetc%2fpasswd"), b"x").unwrap();
    let scan = scan_folder(&stage);
    assert!(scan.rejected.iter().any(|(p, why)| p.ends_with("ibt-0041-0041.sfi") && why.contains("link")), "{:?}", scan.rejected);
    assert!(scan.rejected.iter().any(|(p, why)| p.contains("passwd") && why.contains("name")), "{:?}", scan.rejected);
    assert!(find_in_folder(&stage, "intel/ibt-0041-0041.sfi").is_none(), "a link is not a file");
    assert!(find_in_folder(&stage, "../secret.txt").is_none() && find_in_folder(&stage, "/etc/passwd").is_none());
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    assert!(run_with(&mut env, &m, &data, &Source::Folder(stage.clone()), &Opts::default()).is_err());
    assert!(env.installed.is_empty());
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&stage);
}

#[test]
fn the_release_tarball_is_read_selectively_and_checked_against_the_pinned_hash() {
    let (data, stick, mut env) = (tmp("fwx-tar-d"), tmp("fwx-tar-s"), Fake::new());
    // a small "release": the needed files plus a lot of others the job must not keep
    let mut entries: Vec<(String, u8, String, u32)> = Vec::new();
    for p in NEEDED {
        entries.push((format!("linux-firmware-{TAG}/{p}"), b'0', String::from_utf8(bytes_of(p)).unwrap(), 0o644));
    }
    entries.push((format!("linux-firmware-{TAG}/amdgpu/huge.bin"), b'0', "x".repeat(5000), 0o644));
    let refs: Vec<(&str, u8, &str, u32)> = entries.iter().map(|(a, b, c, d)| (a.as_str(), *b, c.as_str(), *d)).collect();
    let tar = make_tar(&refs);
    let gz = gzip(&tar);
    let tgz = stick.join(format!("linux-firmware-{TAG}.tar.gz"));
    std::fs::write(&tgz, &gz).unwrap();
    // the picker keeps only the wanted
    let got = pick_from_tarball(&tgz, &|p| NEEDED.contains(&p)).unwrap();
    assert_eq!(got.len(), NEEDED.len());
    assert_eq!(got["i915/tgl_guc_70.bin"], b"guc blob");
    assert!(!got.contains_key("amdgpu/huge.bin"));
    // a manifest whose snapshot hash is this tarball's: accepted; any other tarball: refused
    let mut m = fake_manifest();
    m.snapshot_sha256 = crate::fwpack::file_sha256(&tgz).unwrap();
    accept_in(&m, &data, &selection(&env, &m, &data, &[]), "admin", 5).unwrap();
    let o = run_with(&mut env, &m, &data, &Source::Folder(stick.clone()), &Opts::default()).unwrap();
    assert!(o.installed && env.fetched.is_empty());
    let mut other = fake_manifest();
    other.snapshot_sha256 = "cd".repeat(32);
    let (data2, mut env2) = (tmp("fwx-tar-d2"), Fake::new());
    accept_in(&other, &data2, &selection(&env2, &other, &data2, &[]), "admin", 5).unwrap();
    let e = run_with(&mut env2, &other, &data2, &Source::Folder(stick.clone()), &Opts::default()).unwrap_err();
    assert!(e.contains("not the pinned"), "{e}");
    for d in [&data, &data2, &stick] {
        let _ = std::fs::remove_dir_all(d);
    }
}

// ------------------------------------------------------------------------------------------------ remove, rollback

#[test]
fn remove_deletes_the_extra_firmware_and_keeps_the_acceptance() {
    let (data, m) = (tmp("fwx-remove"), fake_manifest());
    let env = Fake::new();
    let sel = selection(&env, &m, &data, &[]);
    accept_in(&m, &data, &sel, "admin", 5).unwrap();
    let (fs, _) = apply::Fs::for_data(&data);
    let got: BTreeMap<String, Vec<u8>> = sel.unique().keys().map(|p| (p.clone(), bytes_of(p))).collect();
    let p = build_pack(&m, &sel, &got, "w", &[]);
    apply::install_pack(&fs, &iyakki_drivers::run::Dry, PACK_ID, "v", "x", "s", "local", &p).unwrap();
    assert!(installed(&data).is_some());
    std::fs::create_dir_all(data.join("fwextra/download")).unwrap();
    std::fs::write(data.join("fwextra/download/x"), b"123").unwrap();
    assert_eq!(leftovers(&data), 3);
    remove(&data).unwrap();
    assert!(installed(&data).is_none() && leftovers(&data) == 0 && read_outcome(&data).is_none());
    assert!(accepted_in(&m, &data, &sel).is_some(), "the acceptance of the same list stays");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn requests_are_remembered_when_the_kernel_log_rolls_over() {
    let (data, m) = (tmp("fwx-seen"), fake_manifest());
    remember(&data, &parse_log(LOG, &m));
    let mut env = Fake::new();
    env.log = String::new();
    let plan = plan_in(&m, &env, &data);
    assert!(plan.groups.iter().any(|g| g.title == "Intel Wi-Fi"), "{:?}", plan.groups.iter().map(|g| &g.title).collect::<Vec<_>>());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn every_pc_gpu_wifi_ethernet_and_bluetooth_file_of_the_pinned_release_has_a_named_vendor_group() {
    let m = manifest();
    // directories of the pinned release that belong to a GPU, Wi-Fi, Ethernet or Bluetooth chip of an ordinary PC
    let pc = ["amdgpu/", "radeon/", "i915/", "xe/", "nvidia/", "intel/iwlwifi/", "intel/ibt-", "intel/ice/", "e100/", "ath10k/", "ath11k/", "ath12k/", "ath6k/", "ath9k_htc/", "ath3k", "ar3k/", "qca/", "rtw88/", "rtw89/", "rtlwifi/", "rtl_bt/", "rtl_nic/", "mediatek/", "brcm/", "cypress/", "tigon/", "bnx2/", "bnx2x/"];
    let mut n = 0;
    for path in m.files.keys().filter(|p| pc.iter().any(|d| p.starts_with(d))) {
        n += 1;
        assert!(!family_of(path).0.starts_with("other-"), "{path} has no vendor group");
    }
    assert!(n > 500, "{n}");
    for p in ["intel/ice/ddp/ice.pkg", "ath3k-1.fw"] {
        if m.files.contains_key(p) {
            assert!(!family_of(p).0.starts_with("other-"), "{p}");
        }
    }
}

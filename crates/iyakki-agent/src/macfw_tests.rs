use super::*;
use iyakki_drivers::run::Recorder;
use std::sync::Mutex;

fn tmp(tag: &str) -> PathBuf {
    let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-macfw-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// The downloaded images the job keeps on the data volume (`download/<key>/BaseSystem.dmg`).
fn kept_dmgs(data: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(work_dir(data).join("download")).into_iter().flatten().flatten().map(|e| e.path().join("BaseSystem.dmg")).filter(|p| p.exists()).collect()
}

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A fake macOS firmware folder with the names a real Mac mini 2018 has for its own platform, and another platform's.
fn macos_firmware(root: &Path) {
    macos_firmware_with(root, true)
}

fn macos_firmware_with(root: &Path, hcd: bool) {
    let w = "wifi/C-4364__s-B2";
    put(root, &format!("{w}/P-lanai-X0_M-HRPN_V-u__m-7.5.txt"), b"sromrev = 11\nboardtype=0x8a0\n");
    put(root, &format!("{w}/lanai.trx"), b"TRX-LANAI");
    put(root, &format!("{w}/lanai.clmb"), b"CLM-LANAI");
    put(root, &format!("{w}/lanai.txcb"), b"TXCAP-LANAI");
    put(root, &format!("{w}/P-kauai-X1_M-MWAY_V-m__m-9.1.txt"), b"sromrev=11\n");
    put(root, &format!("{w}/kauai.trx"), b"TRX-KAUAI");
    if hcd {
        put(root, "bluetooth/BCM4364B3_1234.hcd", b"HCD-4364");
    }
}

/// A fake macOS firmware folder with the shapes of the BCM4350 / BCM43602 folders (made-up platform names and contents), the plist that
/// lists board `Mac-B4831CEBD52A0C4C` for `alpha`, and a T2 platform too.
fn macos_firmware_board(root: &Path, hcd: bool, plist: bool) {
    let w = "wifi/C-4350__s-C2";
    put(root, &format!("{w}/P-alpha_M-AAAA_V-m__m-1.1.txt"), b"sromrev=11\nboardtype = 0x1\n");
    put(root, &format!("{w}/P-bravo_M-BBBB_V-m__m-2.1.txt"), b"sromrev=11\nboardtype=0x2\n");
    put(root, &format!("{w}/alpha.clmb"), b"CLM-ALPHA");
    put(root, &format!("{w}/alpha.txcb"), b"TXCAP-ALPHA");
    put(root, &format!("{w}/bravo.clmb"), b"CLM-BRAVO");
    if plist {
        put(root, "wifi/alpha-PlatformConfig.plist", b"<plist><string>Mac-B4831CEBD52A0C4C</string><string>Mac-AAAAAAAAAAAAAAAA</string><string>Mac-77EB7D7DAF985301</string></plist>");
        put(root, "wifi/bravo-PlatformConfig.plist", b"<plist><string>Mac-1111111111111111</string></plist>");
    }
    put(root, "wifi/C-4364__s-B2/P-lanai-X0_M-HRPN_V-u__m-7.5.txt", b"sromrev = 11\n");
    put(root, "wifi/C-4364__s-B2/lanai.trx", b"TRX-LANAI");
    if hcd {
        put(root, "bluetooth/BCM4350C0_003.001.013.0100.hcd", b"HCD-4350C0");
        put(root, "bluetooth/BCM4364B3_1234.hcd", b"HCD-4364");
    }
}

/// A tiny Intel HEX file in Apple's shape (a stray first line, an extended linear address, no end-of-file record).
fn synthetic_hex() -> Vec<u8> {
    let rec = |t: u8, addr: u16, data: &[u8]| {
        let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, t];
        b.extend_from_slice(data);
        let sum = b.iter().fold(0u8, |a, x| a.wrapping_add(*x));
        b.push(0u8.wrapping_sub(sum));
        format!(":{}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>())
    };
    format!("$\n{}{}", rec(4, 0, &[0xFF, 0x00]), rec(0, 0x4000, &[1, 2, 3, 4, 5, 6, 7, 8])).into_bytes()
}

const MBP_LOG: &str = "[4.3] Bluetooth: hci0: BCM4350C0 UART 37.4 MHz Gamay USI UHE\n[4.4] Bluetooth: hci0: BCM (003.001.134) build 1532\n[4.5] Bluetooth: hci0: BCM: firmware Patch file not found, tried:\n[4.5] Bluetooth: hci0: BCM: 'brcm/BCM.hcd'\n";

struct Fake {
    product: String,
    board_name: String,
    vendor: String,
    bt_log: String,
    /// The image holds the BCM4350 / BCM43602 shapes instead of the T2 ones.
    board_image: bool,
    /// The image's PlatformConfig.plist files exist.
    plists: bool,
    free: Option<u64>,
    memory: bool,
    seven: Result<PathBuf, String>,
    fetch_fail: Option<String>,
    unpack_fail: Option<String>,
    /// Put the firmware folder into the unpacked image (a image without it tests the error).
    image_has_firmware: bool,
    /// Which `os` choices have a BCM4364 patch file in their image.
    hcd_in: Vec<String>,
    /// The name filter finds nothing (only the full read gives the folder).
    quick_finds_nothing: bool,
    /// Apple sends the same image for both choices.
    same_image: bool,
    current_os: String,
    fetches: Vec<(String, String)>,
    /// The candidates whose identity Apple was asked for.
    id_asks: Vec<String>,
    /// Images (by name) whose BCM4350 patch is the macOS 11 hex file instead of a .hcd.
    hex_in: Vec<String>,
    unpack_calls: Vec<bool>,
    lib_modules_free: Option<u64>,
    fs: apply::Fs,
    run: Recorder,
    looks: Mutex<Vec<Seen>>,
    said: Vec<String>,
    fetched: Vec<String>,
    unpacked: Vec<String>,
    sleeps: u32,
    installs: Vec<String>,
}

impl Fake {
    /// What the fake's images are called: `default` and `latest` for the Mac's own board, `macos11` for the macOS 11 candidate.
    fn image_name(&self, c: &Candidate) -> String {
        if c.board_id == MACOS11_BOARD { "macos11".into() } else { c.os.clone() }
    }
    fn id_for(&self, os: &str) -> String {
        if self.same_image { "012-34567 http://oscdn.apple.com/x.dmg".into() } else { format!("012-34567 http://oscdn.apple.com/{os}.dmg") }
    }
    fn new(root: &Path) -> Fake {
        let fs = apply::Fs::under(root);
        std::fs::create_dir_all(fs.proc.join("sys/kernel")).unwrap();
        std::fs::write(fs.proc.join("sys/kernel/osrelease"), "6.12.112\n").unwrap();
        std::fs::write(fs.proc.join("modules"), "").unwrap();
        Fake {
            product: "Macmini8,1".into(),
            board_name: String::new(),
            vendor: "Apple Inc.".into(),
            bt_log: String::new(),
            board_image: false,
            plists: true,
            free: Some(50 << 30),
            memory: false,
            seven: Ok(PathBuf::from("/fake/7zz")),
            fetch_fail: None,
            unpack_fail: None,
            image_has_firmware: true,
            hcd_in: vec!["default".into()],
            quick_finds_nothing: false,
            same_image: false,
            current_os: "default".into(),
            fetches: vec![],
            id_asks: vec![],
            hex_in: vec![],
            unpack_calls: vec![],
            lib_modules_free: None,
            fs,
            run: Recorder::default(),
            looks: Mutex::new(vec![Seen { wifi: vec!["wlan0".into()], bt_controller: true, bt_address: Some("AA:BB:CC:00:11:22".into()) }]),
            said: vec![],
            fetched: vec![],
            unpacked: vec![],
            sleeps: 0,
            installs: vec![],
        }
    }
    fn data(&self) -> PathBuf {
        self.fs.data.clone()
    }
}

impl Env for Fake {
    fn say(&mut self, line: &str) {
        self.said.push(line.to_string());
    }
    fn product(&self) -> String {
        self.product.clone()
    }
    fn board_name(&self) -> String {
        self.board_name.clone()
    }
    fn vendor(&self) -> String {
        self.vendor.clone()
    }
    fn bt_log(&mut self) -> String {
        self.bt_log.clone()
    }
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        self.free
    }
    fn data_in_memory(&self) -> bool {
        self.memory
    }
    fn ensure_7zz(&mut self) -> Result<PathBuf, String> {
        self.seven.clone()
    }
    fn fetch_image(&mut self, c: &Candidate, dir: &Path) -> Result<(PathBuf, String), String> {
        self.fetched.push(c.board_id.clone());
        self.fetches.push((c.board_id.clone(), c.os.clone()));
        self.current_os = self.image_name(c);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("BaseSystem.dmg"), b"partial").unwrap();
        if let Some(e) = &self.fetch_fail {
            return Err(e.clone());
        }
        Ok((dir.join("BaseSystem.dmg"), self.id_for(&self.image_name(c))))
    }
    fn image_id(&mut self, c: &Candidate) -> Result<String, String> {
        self.id_asks.push(c.key.clone());
        Ok(self.id_for(&self.image_name(c)))
    }
    fn unpack(&mut self, _seven: &Path, image: &Path, out: &Path, full: bool) -> Result<String, String> {
        self.unpacked.push(image.display().to_string());
        self.unpack_calls.push(full);
        if let Some(e) = &self.unpack_fail {
            return Err(e.clone());
        }
        // which fake image this is: by the folder the job keeps it in (a kept image is unpacked again without a fetch)
        let p = image.display().to_string();
        let name = if p.contains(MACOS11_BOARD) { "macos11" } else if p.contains("-latest-") { "latest" } else { "default" }.to_string();
        self.current_os = name.clone();
        if !self.image_has_firmware || (!full && self.quick_finds_nothing) {
            put(out, "4.hfs/usr/bin/true", b"x");
        } else if self.board_image {
            macos_firmware_board(&out.join("macOS Base System/usr/share/firmware"), self.hcd_in.contains(&name), self.plists);
        } else {
            macos_firmware_with(&out.join("macOS Base System/usr/share/firmware"), self.hcd_in.contains(&name));
        }
        if self.hex_in.contains(&name) && self.image_has_firmware {
            put(&out.join("macOS Base System/System/Library/Frameworks/IOBluetooth.framework/Versions/A/Resources"), "BCM4350-Updater.hex", &synthetic_hex());
        }
        Ok("Everything is Ok\n\nFolders: 6\nFiles: 7\n".into())
    }
    fn install(&mut self, id: &str, version: &str, source: &str, p: &pack::Pack) -> Result<apply::Report, String> {
        self.installs.push(id.to_string());
        install_with_unload(&self.fs, &self.run, id, version, source, p)
    }
    fn look(&mut self) -> Seen {
        let mut l = self.looks.lock().unwrap();
        if l.len() > 1 { l.remove(0) } else { l[0].clone() }
    }
    fn kernel_lines(&mut self) -> Vec<String> {
        vec!["brcmfmac 0000:02:00.0: Firmware: BCM4364/3 wl0".into()]
    }
    fn sleep(&mut self, _d: Duration) {
        self.sleeps += 1;
    }
    fn now(&self) -> i64 {
        1_790_000_000
    }
}

fn go(f: &mut Fake, source: &Source) -> Result<Outcome, String> {
    let data = f.data();
    run(f, &data, source)
}

#[test]
fn downloading_for_a_mac_mini_2018_installs_the_wifi_and_bluetooth_packs_and_loads_the_drivers() {
    let root = tmp("dl");
    let mut f = Fake::new(&root);
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.fetched, vec!["Mac-7BA5B2DFE22DDD8C"], "the board id of the Mac mini 2018");
    assert_eq!(f.installs, vec![PACK_WIFI, PACK_BT]);
    // the files, under the names the kernel asks for, only for this Mac's platform
    let wifi = f.fs.data.join("firmware/mac-wifi-brcmfmac/files/brcm");
    let names: Vec<String> = std::fs::read_dir(&wifi).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for ext in ["bin", "txt", "clm_blob", "txcap_blob"] {
        assert!(names.contains(&format!("brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.{ext}")), "{ext}: {names:?}");
    }
    assert!(names.iter().all(|n| !n.contains("kauai")), "another platform's firmware is left out: {names:?}");
    assert_eq!(std::fs::read(wifi.join("brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin")).unwrap(), b"TRX-LANAI");
    assert_eq!(std::fs::read_to_string(wifi.join("brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.txt")).unwrap(), "sromrev= 11\nboardtype=0x8a0\n");
    assert_eq!(std::fs::read(f.fs.data.join("firmware/mac-bt-broadcom/files/brcm/BCM.hcd")).unwrap(), b"HCD-4364");
    // linked where the kernel looks, and a pack.json so the next boot links them again
    assert!(f.fs.fw_root.join("brcm/BCM.hcd").exists() && f.fs.data.join("firmware/mac-wifi-brcmfmac/pack.json").exists());
    // the drivers loaded again
    let calls = f.run.calls.lock().unwrap().clone();
    assert!(calls.contains(&"modprobe brcmfmac".to_string()) && calls.contains(&"modprobe hci_uart".to_string()), "{calls:?}");
    // the result
    assert!(o.installed && o.wifi_files == 4 && o.bt_files == 1);
    assert!(o.headline.contains("Wi-Fi firmware installed: wlan0 appeared") && o.headline.contains("Bluetooth firmware installed") && o.headline.contains("AA:BB:CC:00:11:22"), "{}", o.headline);
    assert_eq!(read_outcome(&f.data()).unwrap(), o);
    // the notice is the first line of the log; temporary files are gone
    assert!(f.said[0].contains("These files are Apple's") && f.said[0].contains("never uploaded"));
    assert!(!work_dir(&f.data()).join("download").exists() && !work_dir(&f.data()).join("unpacked").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_packs_come_back_at_the_next_boot_and_load_the_drivers_by_name() {
    let root = tmp("boot");
    let mut f = Fake::new(&root);
    go(&mut f, &Source::Download).unwrap();
    // a restart: /lib/firmware is empty again, nothing is loaded
    std::fs::remove_dir_all(&f.fs.fw_root).unwrap();
    let boot = Recorder::default();
    let log = apply::apply_boot(&f.fs, &boot);
    assert!(f.fs.fw_root.join("brcm/brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin").exists(), "{log:?}");
    let mut calls = boot.calls.lock().unwrap().clone();
    calls.sort();
    assert_eq!(calls, vec!["modprobe brcmfmac", "modprobe hci_uart"], "explicit loads, which a blacklist does not stop");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_wifi_driver_already_loaded_with_its_vendor_module_is_unloaded_in_the_right_order() {
    let root = tmp("wcc");
    let mut f = Fake::new(&root);
    std::fs::write(f.fs.proc.join("modules"), "brcmfmac_wcc 16384 0 - Live 0x0\nbrcmfmac 400000 1 brcmfmac_wcc, Live 0x0\n").unwrap();
    go(&mut f, &Source::Download).unwrap();
    let calls = f.run.calls.lock().unwrap().clone();
    let pos = |c: &str| calls.iter().position(|x| x == c).unwrap_or_else(|| panic!("{c} not in {calls:?}"));
    assert!(pos("modprobe -r brcmfmac_wcc") < pos("modprobe -r brcmfmac") && pos("modprobe -r brcmfmac") < pos("modprobe brcmfmac"), "{calls:?}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_model_that_is_not_in_the_table_is_told_so_and_nothing_is_downloaded() {
    let root = tmp("model");
    let mut f = Fake::new(&root);
    f.product = "Macmini7,1".into();
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("not in IYAKKI's table") && e.contains("macOS script"), "{e}");
    assert!(f.fetched.is_empty() && f.installs.is_empty());
    let o = read_outcome(&f.data()).unwrap();
    assert!(!o.installed && o.headline.starts_with("Not done"), "{o:?}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_unverified_board_id_is_said_before_the_download_and_the_models_own_files_are_used() {
    let root = tmp("unverified");
    let mut f = Fake::new(&root);
    f.product = "MacBookPro15,1".into();
    go(&mut f, &Source::Download).unwrap();
    assert!(f.said.iter().any(|l| l.contains("comes from a list, not from a run on that Mac")), "{:?}", f.said);
    assert_eq!(f.fetched, vec!["Mac-937A206F2EE63C01"]);
    // no platform filter for a model whose platform was not seen: both platforms' sets are taken
    let names: Vec<String> = std::fs::read_dir(f.fs.data.join("firmware/mac-wifi-brcmfmac/files/brcm")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().any(|n| n.contains("kauai")) && names.iter().any(|n| n.contains("lanai")));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn too_little_free_space_or_a_ram_data_volume_stops_before_the_download() {
    let root = tmp("space");
    let mut f = Fake::new(&root);
    f.free = Some(800 << 20);
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("800 MB free") && e.contains("needs about 2560 MB"), "{e}");
    assert!(f.fetched.is_empty());
    let mut f = Fake::new(&root);
    f.memory = true;
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("in memory") && e.contains("macOS script"), "{e}");
    assert!(f.fetched.is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_failed_download_keeps_the_partial_file_for_the_retry_and_a_good_run_removes_it() {
    let root = tmp("retry");
    let mut f = Fake::new(&root);
    f.fetch_fail = Some("the connection dropped".into());
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("dropped"));
    assert_eq!(kept_dmgs(&f.data()).len(), 1, "kept to resume");
    f.fetch_fail = None;
    go(&mut f, &Source::Download).unwrap();
    assert!(!work_dir(&f.data()).join("download").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_disk_image_the_person_picked_is_read_and_never_deleted_or_downloaded() {
    let root = tmp("image");
    let mut f = Fake::new(&root);
    let img = root.join("macOS.dmg");
    std::fs::write(&img, b"dmg").unwrap();
    go(&mut f, &Source::Image(img.clone())).unwrap();
    assert!(f.fetched.is_empty() && f.unpacked == vec![img.display().to_string()]);
    assert!(img.exists(), "the person's file stays");
    let e = go(&mut f, &Source::Image(root.join("none.dmg"))).unwrap_err();
    assert!(e.contains("is not a file"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_image_without_the_firmware_folder_and_a_7zip_failure_are_plain_errors_with_a_clean_work_folder() {
    let root = tmp("badimg");
    let mut f = Fake::new(&root);
    f.image_has_firmware = false;
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("no usr/share/firmware") && e.contains("kept for a look"), "{e}");
    assert!(work_dir(&f.data()).join("unpacked").exists() && f.installs.is_empty(), "kept for a look");
    let mut f = Fake::new(&root);
    f.unpack_fail = Some("7-Zip could not read the disk image (exit 2): Unsupported Method".into());
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("Unsupported Method"));
    let mut f = Fake::new(&root);
    f.seven = Err("7-Zip could not be installed: no network".into());
    assert!(go(&mut f, &Source::Download).unwrap_err().contains("no network"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_folder_the_macos_script_wrote_is_installed_as_it_is() {
    let root = tmp("script");
    let mut f = Fake::new(&root);
    let stick = root.join("stick/iyakki-wifi-firmware");
    put(&stick, "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin", b"TRX");
    put(&stick, "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.txt", b"sromrev=11\n");
    put(&stick, "BCM.hcd", b"HCD");
    put(&stick, "raw/notes.txt", b"ignored");
    let o = go(&mut f, &Source::Folder(root.join("stick"))).unwrap();
    assert!(f.fetched.is_empty() && f.unpacked.is_empty(), "no download, no 7-Zip");
    assert_eq!((o.wifi_files, o.bt_files), (2, 1));
    assert!(f.said.iter().any(|l| l.contains("already named for Linux")));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_copy_of_macos_firmware_folder_on_a_stick_is_named_by_the_rust_extractor() {
    let root = tmp("copy");
    let mut f = Fake::new(&root);
    macos_firmware(&root.join("stick/Macintosh HD/usr/share/firmware"));
    let o = go(&mut f, &Source::Folder(root.join("stick"))).unwrap();
    assert_eq!((o.wifi_files, o.bt_files), (4, 1));
    assert!(f.said.iter().any(|l| l.contains("found macOS's firmware folder at")));
    // a folder with nothing useful
    let e = go(&mut f, &Source::Folder(root.join("empty"))).unwrap_err();
    assert!(e.contains("is not a folder"));
    std::fs::create_dir_all(root.join("empty")).unwrap();
    assert!(go(&mut f, &Source::Folder(root.join("empty"))).unwrap_err().contains("no firmware found"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn bluetooth_without_an_address_is_reported_as_the_known_limit_and_a_missing_wifi_interface_as_a_restart_hint() {
    let root = tmp("limit");
    let mut f = Fake::new(&root);
    *f.looks.lock().unwrap() = vec![Seen { wifi: vec!["wlan0".into()], bt_controller: true, bt_address: None }];
    let o = go(&mut f, &Source::Download).unwrap();
    assert!(o.headline.contains("still has no address"), "{}", o.headline);
    assert!(o.lines.iter().any(|l| l.contains("F-NET-11") && l.contains("USB Bluetooth adapter")));
    assert!(f.sleeps >= 19, "it waited for the address: {}", f.sleeps);
    let mut f = Fake::new(&root);
    *f.looks.lock().unwrap() = vec![Seen::default()];
    let o = go(&mut f, &Source::Download).unwrap();
    assert!(o.installed && o.headline.contains("no wireless interface has appeared yet") && o.headline.contains("no Bluetooth controller appeared"), "{}", o.headline);
    assert!(o.lines.iter().any(|l| l.contains("restart IYAKKI once")));
    assert!(o.kernel.iter().any(|l| l.contains("brcmfmac")), "kernel lines are kept for the page");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn files_without_bluetooth_firmware_install_wifi_only_and_say_so() {
    let root = tmp("wifionly");
    let mut f = Fake::new(&root);
    let stick = root.join("stick");
    put(&stick, "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin", b"TRX");
    put(&stick, "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.txt", b"x=1\n");
    let o = go(&mut f, &Source::Folder(stick)).unwrap();
    assert_eq!(f.installs, vec![PACK_WIFI]);
    assert!(o.headline.contains("no Bluetooth firmware was in these files"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_firmware_file_too_big_for_a_pack_is_refused_whole_instead_of_installing_half_a_set() {
    let files = vec![("brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin".to_string(), vec![1u8; pack::MAX_FILE_BYTES + 1]), ("brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.txt".to_string(), b"x=1\n".to_vec())];
    let e = build_packs(&files, "test").unwrap_err();
    assert!(e.contains("larger than a driver pack file may be") && e.contains("Nothing was installed") || e.contains("nothing was installed"), "{e}");
}

#[test]
fn sources_round_trip_through_the_job_arguments() {
    for s in [Source::Download, Source::Image("/mnt/usb/a b.dmg".into()), Source::Folder("/mnt/usb".into())] {
        assert_eq!(Source::from_args(&s.args()).unwrap(), s);
    }
    assert_eq!(Source::from_args(&[]).unwrap(), Source::Download);
    assert!(Source::from_args(&["--source".into(), "image".into()]).unwrap_err().contains("--path"));
    assert!(Source::from_args(&["--source".into(), "nfs".into()]).unwrap_err().contains("unknown source"));
}

#[test]
fn the_machine_is_read_from_sys_the_mount_table_and_the_kernel_log() {
    let root = tmp("look");
    put(&root, "class/net/wlan0/wireless/x", b"");
    put(&root, "class/net/eth0/device/x", b"");
    put(&root, "class/bluetooth/hci0/address", b"00:00:00:00:00:00\n");
    let s = look_in(&root);
    assert_eq!((s.wifi.clone(), s.bt_controller, s.bt_address.clone()), (vec!["wlan0".to_string()], true, None), "an all-zero address is not an address");
    put(&root, "class/bluetooth/hci0/address", b"AC:DE:48:00:11:22\n");
    assert_eq!(look_in(&root).bt_address.as_deref(), Some("AC:DE:48:00:11:22"));
    let mounts = "rootfs / rootfs rw 0 0\n/dev/nvme0n1p3 /var/lib/iyakki ext4 rw 0 0\ntmpfs /run tmpfs rw 0 0\n";
    assert!(!in_memory(mounts, Path::new("/var/lib/iyakki")) && in_memory(mounts, Path::new("/run/x")) && in_memory(mounts, Path::new("/home/a")), "no data mount: the root is RAM");
    let log = "[1.0] usb 1-1: new device\n[2.0] brcmfmac 0000:02:00.0: Direct firmware load for brcm/x.bin failed with error -2\n[3.0] Bluetooth: hci0: BCM: chip id 123\n";
    assert_eq!(interesting(log).len(), 2);
    assert_eq!(version_for(1_790_000_000), "1.20717.853");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_prompt_the_button_copies_is_the_one_in_the_setup_guide_and_keeps_its_safety_rules() {
    let doc = include_str!("../../../docs/MAC-WIFI-BLUETOOTH-SETUP.md");
    assert!(doc.contains(CHAT_PROMPT.trim_end()), "docs/MAC-WIFI-BLUETOOTH-SETUP.md carries the same prompt as macfw_prompt.txt");
    for w in ["Never write to an internal disk", "Never print, copy or send tokens", "Do not guess which .hcd file", "ROLLBACK", "Mac drivers"] {
        assert!(CHAT_PROMPT.contains(w), "{w}");
    }
    // the guide says plainly what is and is not verified, and has the real-device list
    for w in ["Never run against Apple's real service", "**Not verified", "Real-device test checklist", "F-NET-11", "iyakki-t2-holdback.conf"] {
        assert!(doc.contains(w), "{w}");
    }
}

fn held_file(f: &Fake) -> PathBuf {
    let p = f.fs.lib_modules.join("6.12.112/kernel/drivers/bluetooth/hci_uart.ko.iyakki-held");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"ko").unwrap();
    p
}

#[test]
fn the_held_driver_files_are_given_back_when_the_pack_loads_them() {
    let root = tmp("held");
    let mut f = Fake::new(&root);
    let held = held_file(&f);
    go(&mut f, &Source::Download).unwrap();
    assert!(!held.exists() && f.fs.lib_modules.join("6.12.112/kernel/drivers/bluetooth/hci_uart.ko").exists(), "restored before modprobe");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_name_filter_that_finds_nothing_falls_back_to_reading_the_whole_image_when_there_is_room() {
    let root = tmp("full");
    let mut f = Fake::new(&root);
    f.quick_finds_nothing = true;
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.unpack_calls, vec![false, true]);
    assert!(f.said.iter().any(|l| l.contains("reading the whole image without its big folders")));
    assert!(o.installed && o.wifi_files == 4);
    // not enough room for that: a plain error, the download and the folder stay
    let mut g = Fake::new(&root);
    g.quick_finds_nothing = true;
    g.free = Some(2800 << 20);
    let e = go(&mut g, &Source::Download).unwrap_err();
    assert!(e.contains("reading the whole image needs about 3584 MB free"), "{e}");
    assert_eq!(kept_dmgs(&g.data()).len(), 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_seven_zip_command_filters_by_name_or_leaves_out_the_big_folders() {
    let q = unpack_argv(Path::new("/x/7zz"), Path::new("/d/BaseSystem.dmg"), Path::new("/o"), false);
    assert_eq!(&q[..6], ["/x/7zz", "x", "-y", "-bd", "-aoa", "-o/o"]);
    assert!(q.contains(&"-ir!*usr/share/firmware/wifi/*".to_string()) && q.contains(&"-ir!*usr/share/firmware/bluetooth/*".to_string()) && q.contains(&"-ir!*BCM4364*.hcd".to_string()));
    assert!(q.contains(&"-ir!*IOBluetooth.framework/Versions/A/Resources/BCM*-Updater.hex".to_string()), "the macOS 11 hex patches are read too");
    let f = unpack_argv(Path::new("/x/7zz"), Path::new("/d/BaseSystem.dmg"), Path::new("/o"), true);
    assert!(f.contains(&"-xr!System".to_string()) && !f.iter().any(|a| a.starts_with("-ir!")));
}

#[test]
fn a_bluetooth_file_missing_from_the_first_image_is_looked_for_in_the_other_macos_version() {
    let root = tmp("other");
    let mut f = Fake::new(&root);
    f.hcd_in = vec!["latest".into()];
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.fetches.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["default", "latest"]);
    assert_eq!(std::fs::read(f.fs.data.join("firmware/mac-bt-broadcom/files/brcm/BCM.hcd")).unwrap(), b"HCD-4364");
    assert!(f.said.iter().any(|l| l.contains("another download of about 1 GB")), "the cost is said first: {:?}", f.said);
    assert!(o.bt_files == 1 && !work_dir(&f.data()).join("download").exists());
    // Wi-Fi came from the first image, not the second
    assert_eq!(o.wifi_files, 4);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn when_apple_sends_the_same_image_or_the_other_one_has_nothing_the_job_says_so_and_still_installs_wifi() {
    let root = tmp("nohcd");
    let mut f = Fake::new(&root);
    f.hcd_in = vec![];
    f.same_image = true;
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.fetches.len(), 1, "no second download of the same image");
    assert!(f.said.iter().any(|l| l.contains("the same image")) && o.bt_files == 0 && o.wifi_files == 4);
    assert!(o.headline.contains("Apple's macOS versions for this Mac have no firmware for it") && o.headline.contains("USB Bluetooth adapter"), "{}", o.headline);
    let mut g = Fake::new(&root);
    g.hcd_in = vec![];
    let o = go(&mut g, &Source::Download).unwrap();
    assert_eq!(g.fetches.len(), 2);
    assert!(g.said.iter().any(|l| l.contains("the other macOS version Apple offers for this Mac: nothing for this Mac")), "{:?}", g.said);
    assert!(o.installed && o.bt_files == 0);
    // a second image that cannot be fetched is a note, not a failure
    let mut h = Fake::new(&root);
    h.hcd_in = vec![];
    h.free = Some(3000 << 20);
    let o = go(&mut h, &Source::Download).unwrap();
    assert!(o.installed);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn macs_with_pcie_bluetooth_and_disk_images_do_not_ask_for_a_second_version() {
    let root = tmp("pcie");
    let mut f = Fake::new(&root);
    f.product = "MacBookPro15,4".into();
    f.hcd_in = vec![];
    go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.fetches.len(), 1);
    let mut g = Fake::new(&root);
    g.hcd_in = vec![];
    let img = root.join("a.dmg");
    std::fs::write(&img, b"x").unwrap();
    let o = go(&mut g, &Source::Image(img)).unwrap();
    assert!(g.fetches.is_empty() && o.lines.iter().any(|l| l.contains("no BCM4364 patch file")), "{:?}", o.lines);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn after_a_failure_the_download_and_the_unpacked_folder_stay_and_delete_download_clears_them() {
    let root = tmp("keep");
    let mut f = Fake::new(&root);
    f.image_has_firmware = false;
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("is kept for a look") && e.contains("entries"), "{e}");
    let d = f.data();
    assert!(!kept_dmgs(&d).is_empty() && work_dir(&d).join("unpacked").exists());
    assert!(leftovers(&d) > 0);
    let o = read_outcome(&d).unwrap();
    assert!(o.lines.iter().any(|l| l.contains("kept for a retry or a look")), "{:?}", o.lines);
    assert!(delete_leftovers(&d) > 0);
    assert!(!work_dir(&d).join("download").exists() && !work_dir(&d).join("unpacked").exists() && leftovers(&d) == 0);
    // success leaves nothing behind
    let mut g = Fake::new(&root);
    go(&mut g, &Source::Download).unwrap();
    assert_eq!(leftovers(&g.data()), 0);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_mac_mini_whose_macos_has_no_bcm4364_file_is_told_plainly_and_a_pcie_mac_is_not() {
    let root = tmp("nofile");
    let mut f = Fake::new(&root);
    f.hcd_in = vec![];
    let o = go(&mut f, &Source::Download).unwrap();
    assert!(o.lines.iter().any(|l| l == NO_BCM4364_FILE), "{:?}", o.lines);
    for w in ["no firmware file in macOS", "ROM state", "USB Bluetooth adapter is the reliable option"] {
        assert!(NO_BCM4364_FILE.contains(w), "{w}");
    }
    let mut g = Fake::new(&root);
    g.product = "MacBookPro15,4".into();
    g.hcd_in = vec![];
    let o = go(&mut g, &Source::Download).unwrap();
    assert!(!o.lines.iter().any(|l| l == NO_BCM4364_FILE), "the PCIe chip is not the UART one");
    // with the file, nothing is said
    let mut h = Fake::new(&root);
    let o = go(&mut h, &Source::Download).unwrap();
    assert!(!o.lines.iter().any(|l| l == NO_BCM4364_FILE));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn load_bluetooth_anyway_in_a_sandbox_runs_no_program_and_says_nothing_was_held() {
    let root = tmp("anyway");
    let lines = load_bluetooth_anyway(&root);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("Nothing was held"), "{lines:?}");
    assert!(!bluetooth_held(&root.join("none")));
    assert!(LOAD_ANYWAY_HELP.contains("cannot pair") && LOAD_ANYWAY_HELP.contains("Bluetooth stack"));
    std::fs::remove_dir_all(&root).unwrap();
}

// ---------------------------------------------------------------------------------------- a Mac without a T2 chip

/// The real MacBookPro14,1 of the owner (facts from its DMI and kernel log); the files are fakes.
fn macbook(root: &Path) -> Fake {
    let mut f = Fake::new(root);
    f.product = "MacBookPro14,1".into();
    f.board_name = "Mac-B4831CEBD52A0C4C".into();
    f.bt_log = MBP_LOG.into();
    f.board_image = true;
    // this model's own macOS is not downloaded: macOS 11 is the one that has the Bluetooth patch
    f.hcd_in = vec!["macos11".into()];
    f
}

fn fw_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn a_macbook_pro_2017_without_a_t2_chip_asks_apple_for_the_board_id_of_its_dmi_and_installs_the_board_files() {
    let root = tmp("mbp");
    let mut f = macbook(&root);
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.fetched, vec![MACOS11_BOARD], "this model's own macOS has none of its chip files (opened on the real Mac): only macOS 11 is fetched");
    assert!(f.said.iter().any(|l| l.contains("is not downloaded; macOS 11 is the version that keeps the Bluetooth patch")), "{:?}", f.said);
    assert_eq!(f.installs, vec![PACK_WIFI, PACK_BT]);
    let names = fw_names(&f.fs.data.join("firmware/mac-wifi-brcmfmac/files/brcm"));
    let base = "brcmfmac4350c2-pcie.Apple Inc.-MacBookPro14,1";
    assert_eq!(names, vec![format!("{base}.clm_blob"), format!("{base}.txcap_blob"), format!("{base}.txt")], "no .bin (linux-firmware has it), nothing of another platform or the T2 chip");
    let wifi = f.fs.data.join("firmware/mac-wifi-brcmfmac/files/brcm");
    assert_eq!(std::fs::read_to_string(wifi.join(format!("{base}.txt"))).unwrap(), "sromrev=11\nboardtype= 0x1\n");
    assert_eq!(std::fs::read(wifi.join(format!("{base}.clm_blob"))).unwrap(), b"CLM-ALPHA");
    // linked where the kernel asks for them (a name with a space)
    assert!(f.fs.fw_root.join(format!("brcm/{base}.txt")).exists() && f.fs.fw_root.join("brcm/BCM.hcd").exists());
    assert_eq!(std::fs::read(f.fs.data.join("firmware/mac-bt-broadcom/files/brcm/BCM.hcd")).unwrap(), b"HCD-4350C0");
    let calls = f.run.calls.lock().unwrap().clone();
    assert!(calls.contains(&"modprobe brcmfmac".to_string()) && calls.contains(&"modprobe hci_uart".to_string()), "{calls:?}");
    // what it said first
    assert!(f.said.iter().any(|l| l.starts_with("This Mac has no T2 chip") && l.contains("BCM4350") && l.contains("Wi-Fi works without them")), "{:?}", f.said);
    assert!(f.said.iter().any(|l| l.contains("BCM4350C0") && l.contains("brcm/BCM.hcd")));
    assert!(!f.said.iter().any(|l| l.contains("verified list")), "this model is verified");
    assert!(o.installed && o.wifi_files == 3 && o.bt_files == 1, "{o:?}");
    // the boot after: links again, explicit loads
    std::fs::remove_dir_all(&f.fs.fw_root).unwrap();
    let boot = Recorder::default();
    apply::apply_boot(&f.fs, &boot);
    assert!(f.fs.fw_root.join(format!("brcm/{base}.clm_blob")).exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_model_that_is_not_on_the_verified_list_is_allowed_and_told_so() {
    let root = tmp("unknownmodel");
    let mut f = macbook(&root);
    f.product = "MacBookPro12,1".into();
    f.hcd_in = vec!["default".into()];
    f.board_name = "Mac-77EB7D7DAF985301".into();
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.fetched, vec!["Mac-77EB7D7DAF985301"]);
    assert!(f.said.iter().any(|l| l.contains("MacBookPro12,1 is not on IYAKKI's verified list") && l.contains("this Mac's own (DMI board_name)")), "{:?}", f.said);
    // the file is named after this Mac's DMI model, whatever the model
    assert!(fw_names(&f.fs.data.join("firmware/mac-wifi-brcmfmac/files/brcm")).iter().all(|n| n.starts_with("brcmfmac4350c2-pcie.Apple Inc.-MacBookPro12,1.")));
    assert!(o.installed);
    // a firmware that disagrees with the table: the firmware wins, and it is said
    let mut g = macbook(&root);
    g.board_name = "Mac-77EB7D7DAF985301".into();
    go(&mut g, &Source::Download).unwrap();
    assert!(g.said.iter().any(|l| l.contains("this Mac's firmware says Mac-77EB7D7DAF985301")), "{:?}", g.said);
    assert_eq!(g.fetched, vec![MACOS11_BOARD], "the model's own image is skipped; macOS 11 is asked for under its own board id");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn without_a_readable_board_id_and_a_model_in_the_table_the_download_is_refused_but_a_folder_still_works() {
    let root = tmp("noboard");
    let mut f = macbook(&root);
    f.product = "MacBookPro99,9".into();
    f.board_name = "not an apple board".into();
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("board id could not be read") && e.contains("not in IYAKKI's table") && e.contains("macOS script"), "{e}");
    assert!(f.fetched.is_empty() && f.installs.is_empty());
    // a table model without a DMI board name falls back to the table (the Mac mini)
    let mut g = Fake::new(&root);
    g.board_name = String::new();
    go(&mut g, &Source::Download).unwrap();
    assert_eq!(g.fetched, vec!["Mac-7BA5B2DFE22DDD8C"]);
    // a disk image of this Mac: the board id is not needed, but the Wi-Fi part says it cannot tell the platform
    let img = root.join("a.dmg");
    std::fs::write(&img, b"x").unwrap();
    let mut h = macbook(&root);
    h.product = "MacBookPro99,9".into();
    h.board_name = String::new();
    h.hcd_in = vec!["default".into()];
    let o = go(&mut h, &Source::Image(img)).unwrap();
    assert!(h.fetches.is_empty() && h.installs == vec![PACK_BT], "{:?}", h.installs);
    assert!(o.lines.iter().any(|l| l.contains("board id (DMI board_name) could not be read")), "{:?}", o.lines);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn when_the_platform_of_the_board_cannot_be_told_only_bluetooth_is_installed_and_wifi_keeps_its_generic_firmware() {
    let root = tmp("noplatform");
    let mut f = macbook(&root);
    f.plists = false;
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.installs, vec![PACK_BT], "no Wi-Fi pack");
    assert!(o.installed && o.wifi_files == 0 && o.bt_files == 1);
    assert!(o.lines.iter().any(|l| l.contains("cannot tell which platform") && l.contains("nothing was guessed")), "{:?}", o.lines);
    assert!(o.lines.iter().any(|l| l.contains("generic firmware keeps working")), "{:?}", o.lines);
    assert!(o.headline.contains("no extra files for this chip in Apple's macOS"), "{}", o.headline);
    // nothing at all: a plain error
    let mut g = macbook(&root);
    g.plists = false;
    g.hcd_in = vec![];
    g.bt_log = String::new();
    let img = root.join("b.dmg");
    std::fs::write(&img, b"x").unwrap();
    let e = go(&mut g, &Source::Image(img)).unwrap_err();
    assert!(e.contains("no file that"), "{e}");
    assert!(g.installs.is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_other_macos_version_is_asked_for_the_controllers_patch_file_and_a_missing_chip_name_skips_bluetooth() {
    let root = tmp("mbpother");
    let mut f = macbook(&root);
    f.product = "MacBookPro12,1".into();
    f.hcd_in = vec!["latest".into()];
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!(f.fetches.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), vec!["default", "latest"]);
    assert!(f.said.iter().any(|l| l.contains("has no patch file for the BCM4350C0 controller; looking at the next macOS version")), "{:?}", f.said);
    assert_eq!(std::fs::read(f.fs.data.join("firmware/mac-bt-broadcom/files/brcm/BCM.hcd")).unwrap(), b"HCD-4350C0");
    assert_eq!(o.bt_files, 1);
    // neither has it: the plain sentence with the chip's name
    let mut g = macbook(&root);
    g.hcd_in = vec![];
    let o = go(&mut g, &Source::Download).unwrap();
    assert!(o.lines.iter().any(|l| l.contains("BCM4350C0") && l.contains("ROM state") && l.contains("USB Bluetooth adapter")), "{:?}", o.lines);
    assert_eq!(g.installs, vec![PACK_WIFI], "Wi-Fi only");
    // no controller in the log: no patch file is looked for and no second download is made
    let mut h = macbook(&root);
    h.bt_log = String::new();
    h.hcd_in = vec![];
    let o = go(&mut h, &Source::Download).unwrap();
    assert_eq!(h.fetches.len(), 1);
    assert!(h.said.iter().any(|l| l.contains("no UART Bluetooth controller was seen")), "{:?}", h.said);
    assert!(o.installed);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_name_btbcm_tried_first_is_the_name_the_patch_file_is_served_under() {
    let root = tmp("serve");
    let mut f = macbook(&root);
    f.bt_log = "[1] Bluetooth: hci0: BCM4350C0 UART 37.4 MHz x\n[2] Bluetooth: hci0: BCM: 'brcm/BCM4350C0.hcd'\n[2] Bluetooth: hci0: BCM: 'brcm/BCM.hcd'\n".into();
    go(&mut f, &Source::Download).unwrap();
    assert!(f.fs.data.join("firmware/mac-bt-broadcom/files/brcm/BCM4350C0.hcd").exists());
    let calls = f.run.calls.lock().unwrap().clone();
    assert!(calls.contains(&"modprobe hci_uart".to_string()), "a UART controller's file loads the UART driver: {calls:?}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_t2_mac_is_still_a_t2_mac_whatever_its_board_name_says() {
    let root = tmp("t2still");
    let mut f = Fake::new(&root);
    f.board_name = "Mac-7BA5B2DFE22DDD8C".into();
    f.bt_log = MBP_LOG.into();
    go(&mut f, &Source::Download).unwrap();
    assert!(!f.said.iter().any(|l| l.contains("no T2 chip")), "{:?}", f.said);
    assert_eq!(f.installs, vec![PACK_WIFI, PACK_BT]);
    assert!(f.fs.data.join("firmware/mac-bt-broadcom/files/brcm/BCM.hcd").exists(), "the BCM4364 rule, not the log's chip");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_plan_for_a_mac_without_t2_says_what_is_fetched_and_that_wifi_works_without_it() {
    for w in ["no T2 chip", "BCM4350 / BCM43602", "NVRAM and CLM", "Bluetooth patch file", "UART chip", "works without them", "fewer channels"] {
        assert!(NON_T2_PLAN.contains(w), "{w}");
    }
    assert!(no_hcd_message("BCM4350C0").contains("BCM4350C0") && no_hcd_message("BCM4364") == NO_BCM4364_FILE);
}

// ---------------------------------------------------------------------------------------- which images, and never twice

fn macos11_key() -> String {
    Candidate::new("x", MACOS11_BOARD, crate::macrecovery::MLB_ZERO, "default").key
}

#[test]
fn the_candidate_list_depends_on_the_model_and_is_short() {
    let own = "Mac-B4831CEBD52A0C4C";
    let keys = |v: &[Candidate]| v.iter().map(|c| (c.board_id.clone(), c.os.clone())).collect::<Vec<_>>();
    // the 2017 MacBook Pro: its own macOS was opened on the real Mac and has nothing for it: only macOS 11
    let v = candidates("MacBookPro14,1", own, Some("BCM4350C0"));
    assert_eq!(keys(&v), vec![(MACOS11_BOARD.to_string(), "default".to_string())]);
    // an unverified model with the same chip: its own two versions first, then macOS 11; never more than MAX_IMAGES
    let v = candidates("MacBookPro12,1", "Mac-AAAAAAAAAAAAAAAA", Some("BCM4350C0"));
    assert_eq!(keys(&v), vec![("Mac-AAAAAAAAAAAAAAAA".to_string(), "default".to_string()), ("Mac-AAAAAAAAAAAAAAAA".to_string(), "latest".to_string()), (MACOS11_BOARD.to_string(), "default".to_string())]);
    assert!(v.len() <= MAX_IMAGES);
    // the Mac mini: its own two versions, no macOS 11 (its chip's hex patches come in two builds and one cannot be chosen)
    let v = candidates("Macmini8,1", "Mac-7BA5B2DFE22DDD8C", Some("BCM4364"));
    assert_eq!(keys(&v).iter().map(|k| k.1.as_str()).collect::<Vec<_>>(), vec!["default", "latest"]);
    // no UART controller seen: the model's own images only; no board id: nothing
    assert_eq!(candidates("MacBookPro14,1", own, None).len(), 2);
    assert!(candidates("X", "", None).is_empty());
    // keys are folder names
    assert!(candidates("Macmini8,1", "Mac-7BA5B2DFE22DDD8C", None).iter().all(|c| c.key.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')));
    assert_ne!(v[0].key, v[1].key);
}

#[test]
fn a_retry_reads_the_kept_image_and_asks_apple_for_nothing_again() {
    let root = tmp("noredo");
    let mut f = macbook(&root);
    f.seven = Err("7-Zip could not be installed: no network".into());
    let e = go(&mut f, &Source::Download).unwrap_err();
    assert!(e.contains("no network"));
    assert_eq!((f.fetches.len(), f.id_asks.len()), (1, 1), "{:?}", f.said);
    let d = f.data();
    let dir = work_dir(&d).join("download").join(macos11_key());
    assert_eq!(kept_image(&dir), Some(7), "complete and marked");
    let st = card_status(&d);
    assert!(st.contains("macOS 11 (Big Sur)") && st.contains("Retry (no new download)") && st.contains("Delete download"), "{st}");
    assert!(has_cache(&d));
    // retry: the image is read from the data volume; no request of any kind goes to Apple
    f.seven = Ok(PathBuf::from("/fake/7zz"));
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!((f.fetches.len(), f.id_asks.len()), (1, 1), "no second download and no second question: {:?}", f.said);
    assert!(f.said.iter().any(|l| l.contains("using the image kept from an earlier try") && l.contains("no new download")), "{:?}", f.said);
    assert!(o.installed && o.bt_files == 1);
    // success deletes the image and the state
    assert!(kept_dmgs(&d).is_empty() && !state_path(&d).exists() && !has_cache(&d) && card_status(&d).is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_image_with_nothing_for_this_mac_is_not_downloaded_or_even_read_again_until_the_rules_change() {
    let root = tmp("nothing");
    let mut f = macbook(&root);
    f.hcd_in = vec![];
    f.plists = false;
    let e = go(&mut f, &Source::Download).unwrap_err();
    // the honest sentence: what was looked in, what works instead
    for w in ["Apple's macOS versions for this Mac do not contain", "macOS 11 (Big Sur) (nothing for this Mac)", "generic firmware", "USB Bluetooth adapter", "BCM4350C0"] {
        assert!(e.contains(w), "{w}: {e}");
    }
    let o = read_outcome(&f.data()).unwrap();
    assert!(!o.installed && o.headline.starts_with("Not done: Apple's macOS versions"), "{}", o.headline);
    assert!(o.lines.iter().any(|l| l.contains("Retry (no new download)")), "{:?}", o.lines);
    assert!(f.installs.is_empty());
    assert_eq!((f.fetches.len(), f.id_asks.len(), f.unpacked.len()), (1, 1, 1));
    let st = read_state(&f.data());
    assert_eq!(st.cands.len(), 1);
    assert_eq!((st.cands[0].status.as_str(), st.cands[0].scan), ("empty", SCAN_VERSION));
    // retry: the same sentence at once, with nothing fetched, asked or unpacked
    let e2 = go(&mut f, &Source::Download).unwrap_err();
    assert_eq!(e2, e);
    assert_eq!((f.fetches.len(), f.id_asks.len(), f.unpacked.len()), (1, 1, 1));
    assert!(f.said.iter().any(|l| l.contains("looked at in an earlier try") && l.contains("not downloaded again")), "{:?}", f.said);
    // the rules changed (an older scan version): the kept image is read again, still no network
    let mut st = read_state(&f.data());
    st.cands[0].scan = SCAN_VERSION - 1;
    write_state(&f.data(), &st);
    f.hex_in = vec!["macos11".into()];
    let o = go(&mut f, &Source::Download).unwrap();
    assert_eq!((f.fetches.len(), f.id_asks.len(), f.unpacked.len()), (1, 1, 2), "read again from the kept image");
    assert!(o.installed && o.bt_files == 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_hex_patch_from_macos_11_becomes_the_bluetooth_hcd_and_the_log_says_how() {
    let root = tmp("hex");
    let mut f = macbook(&root);
    f.hcd_in = vec![];
    f.hex_in = vec!["macos11".into()];
    let o = go(&mut f, &Source::Download).unwrap();
    let want = iyakki_drivers::ihex::to_hcd(&synthetic_hex()).unwrap();
    assert_eq!(std::fs::read(f.fs.data.join("firmware/mac-bt-broadcom/files/brcm/BCM.hcd")).unwrap(), want);
    assert!(f.said.iter().any(|l| l.contains("BCM.hcd was made from BCM4350-Updater.hex") && l.contains("Intel HEX") && l.contains("sha256")), "{:?}", f.said);
    assert!(o.headline.contains("Bluetooth firmware installed: the controller has an address"), "{}", o.headline);
    // the Wi-Fi part is stated as it is: extra files for this chip were found in the fake image
    assert!(o.headline.contains("Wi-Fi extra files installed"), "{}", o.headline);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_partial_download_resumes_without_asking_and_may_start_with_little_free_space() {
    let root = tmp("resume");
    let mut f = macbook(&root);
    f.free = Some(800 << 20);
    let dir = work_dir(&f.data()).join("download").join(macos11_key());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("BaseSystem.dmg"), b"abc").unwrap();
    let o = go(&mut f, &Source::Download).unwrap();
    assert!(f.id_asks.is_empty() && f.fetches.len() == 1, "the partial file is continued: {:?}", f.said);
    assert!(f.said.iter().any(|l| l.contains("continuing a partial file")), "{:?}", f.said);
    assert!(o.installed);
    // a marker whose size does not match the file is not trusted
    let d2 = root.join("m");
    std::fs::create_dir_all(&d2).unwrap();
    std::fs::write(d2.join("BaseSystem.dmg"), b"12345").unwrap();
    assert_eq!(kept_image(&d2), None, "no marker");
    std::fs::write(d2.join("verified"), "9").unwrap();
    assert_eq!(kept_image(&d2), None, "the size differs");
    std::fs::write(d2.join("verified"), "5").unwrap();
    assert_eq!(kept_image(&d2), Some(5));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_download_of_the_older_one_image_layout_is_removed_and_said() {
    let root = tmp("legacy");
    let mut f = macbook(&root);
    let old = work_dir(&f.data()).join("download");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("BaseSystem.dmg"), vec![0u8; 2 << 20]).unwrap();
    f.hcd_in = vec!["macos11".into()];
    go(&mut f, &Source::Download).unwrap();
    assert!(f.said.iter().any(|l| l.contains("removed a download of an older layout (2 MB")), "{:?}", f.said);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_failed_image_read_stops_the_loop_for_tool_errors_but_goes_on_for_image_errors() {
    let root = tmp("fatal");
    // 7-Zip failing is the tool's fault: the second image would fail the same way and is not downloaded
    let mut f = Fake::new(&root);
    f.unpack_fail = Some("7-Zip could not read the disk image (exit 2)".into());
    assert!(go(&mut f, &Source::Download).unwrap_err().contains("exit 2"));
    assert_eq!(f.fetches.len(), 1);
    // an image without the firmware folder is that image's fault: the next version is tried
    let root2 = tmp("fatal2");
    let mut g = Fake::new(&root2);
    g.image_has_firmware = false;
    let e = go(&mut g, &Source::Download).unwrap_err();
    assert_eq!(g.fetches.len(), 2, "both of the Mac mini's versions were tried");
    assert!(e.contains("no usr/share/firmware"), "{e}");
    std::fs::remove_dir_all(&root).unwrap();
    std::fs::remove_dir_all(&root2).unwrap();
}

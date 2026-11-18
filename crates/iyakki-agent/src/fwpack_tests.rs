use super::*;
use std::sync::Mutex;

fn tmp(tag: &str) -> PathBuf {
    let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-fwpack-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn realtek() -> &'static Def {
    def_for_pack("fw-realtek-net").unwrap()
}

/// The bytes of the made-up files and the table that pins them (pack "realtek-net", the real Def).
fn fake_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![("rtl_nic/rtl8168h-2.fw", (0..3000u32).map(|i| (i * 7 % 251) as u8).collect()), ("rtl_nic/rtl8125b-2.fw", (0..2000u32).map(|i| (i * 13 % 239) as u8).collect()), ("LICENSE.r8169", b"Permission is hereby granted for the distribution of this firmware data in hexadecimal or equivalent format".to_vec())]
}

fn fake_table() -> Table {
    let mut s = String::from("commit 1a935bfb330d4a46786eaa903580db9e757b84fa\nbase https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain\npack realtek-net\n");
    for (p, b) in fake_files() {
        let kind = if p.starts_with("LICENSE") { "doc" } else { "file" };
        s.push_str(&format!("{kind} {} {} {p}\n", b.len(), sha256_hex(&b)));
    }
    // another pack, so the table has more than the one under test
    s.push_str(&format!("pack qualcomm-bt\ndoc 3 {} LICENSE.qcom\nfile 3 {} qca/x.bin\n", sha256_hex(b"abc"), sha256_hex(b"abc")));
    parse_table(&s).unwrap()
}

// ------------------------------------------------------------------------------------------------ the pinned table

#[test]
fn the_pinned_table_is_complete_and_every_def_has_files_and_licences() {
    let t = table();
    assert!(t.commit.len() == 40 && t.base.starts_with("https://git.kernel.org/"), "{t:?}");
    for d in DEFS.iter() {
        let p = d.files();
        assert!(!p.files.is_empty() && !p.docs.is_empty(), "{}", d.pack_id);
        assert!(d.pack_id == format!("fw-{}", d.table_id), "pack ids are fw-<table id>");
        // everything must fit a driver pack
        assert!(p.files.len() + p.docs.len() <= pack::MAX_FILES && p.files.iter().chain(&p.docs).map(|e| e.size).sum::<u64>() <= pack::MAX_TOTAL_BYTES as u64 && p.files.iter().all(|e| e.size <= pack::MAX_FILE_BYTES as u64), "{} does not fit a driver pack", d.pack_id);
    }
    assert_eq!(t.packs.len(), DEFS.len());
    // no file is in two packs, and the 20 Qualcomm files are the ones under LICENSE.qcom
    let mut seen = std::collections::BTreeSet::new();
    for p in &t.packs {
        for e in &p.files {
            assert!(seen.insert(e.path.clone()), "{} is in two packs", e.path);
        }
    }
    assert_eq!(def_for_pack("fw-qualcomm-bt").unwrap().files().files.len(), 20);
    assert!(def_for_pack("fw-atheros-carl9170").unwrap().files().files.iter().any(|e| e.path == "carl9170-1.fw"));
    assert!(def_for_pack("fw-realtek-net").unwrap().files().files.iter().all(|e| e.path.starts_with("rtl_nic/")), "the USB adapters' files (r8152) are not in the pack");
    assert!(!def_for_pack("fw-realtek-net").unwrap().files().files.iter().any(|e| e.path.contains("rtl8153") || e.path.contains("rtl8156")));
}

#[test]
fn a_malformed_or_incomplete_table_is_refused() {
    let good = "commit 1a935bfb330d4a46786eaa903580db9e757b84fa\nbase https://x.example/p\npack a\ndoc 3 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad L\nfile 3 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad f/x\n";
    assert!(parse_table(good).is_ok());
    for bad in [good.replace("1a935bfb", "1A935BFB"), good.replace("https://", "http://"), good.replace("f/x", "../x"), good.replace("ba7816bf8f", "ba7816bf8"), good.replace("file 3", "file 0"), good.replace("doc 3", "weird 3"), good.replace("pack a\n", ""), good.replace("doc 3 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad L\n", "")] {
        assert!(parse_table(&bad).is_err(), "{bad}");
    }
}

// ------------------------------------------------------------------------------------------------ hardware

fn put(sys: &Path, rel: &str, v: &str) {
    let p = sys.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, format!("{v}\n")).unwrap();
}

#[test]
fn a_pack_is_shown_only_where_its_hardware_is() {
    let sys = tmp("hw");
    assert!(detect(&sys).is_empty(), "no devices, no cards");
    put(&sys, "bus/pci/devices/0000:03:00.0/vendor", "0x10ec");
    put(&sys, "bus/pci/devices/0000:03:00.0/device", "0x8168");
    put(&sys, "bus/pci/devices/0000:04:00.0/vendor", "0x8086");
    put(&sys, "bus/pci/devices/0000:04:00.0/device", "0x8168");
    assert_eq!(detect(&sys).iter().map(|d| d.pack_id).collect::<Vec<_>>(), ["fw-realtek-net"], "the Realtek NIC, not an Intel device with the same device id");
    // a Qualcomm combo card on PCI, an Atheros USB stick
    put(&sys, "bus/pci/devices/0000:05:00.0/vendor", "0x17cb");
    put(&sys, "bus/pci/devices/0000:05:00.0/device", "0x1103");
    put(&sys, "bus/usb/devices/1-2/idVendor", "0cf3");
    put(&sys, "bus/usb/devices/1-2/idProduct", "9170");
    put(&sys, "bus/usb/devices/1-3/idVendor", "0cf3");
    put(&sys, "bus/usb/devices/1-3/idProduct", "9271");
    let ids: Vec<&str> = detect(&sys).iter().map(|d| d.pack_id).collect();
    assert_eq!(ids, ["fw-qualcomm-bt", "fw-realtek-net", "fw-atheros-carl9170"]);
    // the AR9271 stick (ath9k_htc, its files stay in the images) alone shows nothing
    let sys2 = tmp("hw2");
    put(&sys2, "bus/usb/devices/1-3/idVendor", "0cf3");
    put(&sys2, "bus/usb/devices/1-3/idProduct", "9271");
    assert!(detect(&sys2).is_empty());
    // a Broadcom Tigon3 and a bnx2x card
    put(&sys2, "bus/pci/devices/0000:01:00.0/vendor", "0x14e4");
    put(&sys2, "bus/pci/devices/0000:01:00.0/device", "0x1682");
    assert_eq!(detect(&sys2).iter().map(|d| d.pack_id).collect::<Vec<_>>(), ["fw-broadcom-net"]);
    let _ = std::fs::remove_dir_all(&sys);
    let _ = std::fs::remove_dir_all(&sys2);
}

#[test]
fn a_device_without_a_driver_is_unbound_and_a_built_in_module_is_told_from_a_loaded_one() {
    let sys = tmp("bound");
    assert_eq!(bound_in(&sys, realtek()), None);
    put(&sys, "bus/pci/devices/0000:03:00.0/vendor", "0x10ec");
    put(&sys, "bus/pci/devices/0000:03:00.0/device", "0x8168");
    assert_eq!(bound_in(&sys, realtek()), Some(false));
    std::fs::create_dir_all(sys.join("bus/pci/drivers/r8169")).unwrap();
    std::os::unix::fs::symlink("../../drivers/r8169", sys.join("bus/pci/devices/0000:03:00.0/driver")).unwrap();
    assert_eq!(bound_in(&sys, realtek()), Some(true));
    // USB: bound through an interface
    let q = def_for_pack("fw-atheros-carl9170").unwrap();
    put(&sys, "bus/usb/devices/1-2/idVendor", "0cf3");
    put(&sys, "bus/usb/devices/1-2/idProduct", "9170");
    assert_eq!(bound_in(&sys, q), Some(false));
    std::fs::create_dir_all(sys.join("bus/usb/devices/1-2/1-2:1.0")).unwrap();
    std::fs::create_dir_all(sys.join("bus/usb/drivers/carl9170")).unwrap();
    std::os::unix::fs::symlink("../../../drivers/carl9170", sys.join("bus/usb/devices/1-2/1-2:1.0/driver")).unwrap();
    assert_eq!(bound_in(&sys, q), Some(true));
    // modules
    std::fs::create_dir_all(sys.join("module/r8169")).unwrap();
    assert_eq!(module_state_in(&sys, "r8169"), ModState::Builtin);
    put(&sys, "module/tg3/initstate", "live");
    assert_eq!(module_state_in(&sys, "tg3"), ModState::Loaded);
    assert_eq!(module_state_in(&sys, "bnx2"), ModState::Absent);
    let _ = std::fs::remove_dir_all(&sys);
}

// ------------------------------------------------------------------------------------------------ the job with a fake world

struct Fake {
    present: bool,
    in_memory: bool,
    free: Option<u64>,
    /// url suffix (the path) -> bytes the "network" serves
    net: Vec<(String, Vec<u8>)>,
    fetched: Vec<String>,
    installed: Vec<(String, pack::Pack)>,
    report: Vec<String>,
    mods: Vec<(String, ModState)>,
    bound: Option<bool>,
    said: Vec<String>,
}

impl Fake {
    fn new() -> Fake {
        Fake { present: true, in_memory: false, free: Some(1 << 30), net: fake_files().into_iter().map(|(p, b)| (p.to_string(), b)).collect(), fetched: vec![], installed: vec![], report: vec!["driver r8169 reloaded".into()], mods: vec![("r8169".into(), ModState::Loaded)], bound: Some(true), said: vec![] }
    }
}

impl Env for Fake {
    fn say(&mut self, l: &str) {
        self.said.push(l.to_string());
    }
    fn hardware_present(&self, _d: &Def) -> bool {
        self.present
    }
    fn free_bytes(&self, _d: &Path) -> Option<u64> {
        self.free
    }
    fn data_in_memory(&self) -> bool {
        self.in_memory
    }
    fn fetch(&mut self, url: &str, want: u64, dest: &Path) -> Result<(), String> {
        self.fetched.push(url.to_string());
        let path = url.split("/plain/").nth(1).unwrap().split('?').next().unwrap();
        let (_, b) = self.net.iter().find(|(p, _)| p == path).ok_or(format!("404 {path}"))?;
        assert!(url.ends_with("?id=1a935bfb330d4a46786eaa903580db9e757b84fa"), "the pinned commit is in every address: {url}");
        assert!(want > 0);
        std::fs::write(dest, b).map_err(|e| e.to_string())
    }
    fn install(&mut self, def: &Def, _v: &str, _s: &str, p: &pack::Pack) -> Result<apply::Report, String> {
        self.installed.push((def.pack_id.to_string(), p.clone()));
        Ok(apply::Report { lines: self.report.clone(), cmdline: vec![] })
    }
    fn module_state(&self, n: &str) -> ModState {
        self.mods.iter().find(|m| m.0 == n).map_or(ModState::Absent, |m| m.1)
    }
    fn device_bound(&self, _d: &Def) -> Option<bool> {
        self.bound
    }
    fn now(&self) -> i64 {
        1_700_000_000
    }
}

fn go(env: &mut Fake, data: &Path) -> Result<Outcome, String> {
    run_with(env, &fake_table(), data, realtek(), false, None)
}

fn accept_it(data: &Path) {
    accept_in(&fake_table(), data, realtek(), "admin", 5).unwrap();
}

#[test]
fn a_run_downloads_checks_installs_with_the_licence_and_says_it_plainly() {
    let d = tmp("ok");
    accept_it(&d);
    let mut env = Fake::new();
    let o = go(&mut env, &d).unwrap();
    assert!(o.installed && !o.reboot_needed && o.files == 2 && o.source == "download", "{o:?}");
    assert!(o.headline.contains("installed and the driver was loaded again"), "{}", o.headline);
    assert_eq!(env.installed.len(), 1);
    let (id, p) = &env.installed[0];
    assert_eq!(id, "fw-realtek-net");
    let paths: Vec<&str> = p.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["rtl_nic/rtl8168h-2.fw", "rtl_nic/rtl8125b-2.fw", "licences/fw-realtek-net/LICENSE.r8169"], "the licence text travels with the files");
    assert!(p.validate().is_ok() && p.reload == ["r8169"], "{p:?}");
    // the result is on the volume, the download folder is gone
    assert_eq!(read_outcome(&d, realtek()).unwrap(), o);
    assert_eq!(leftovers(&d, realtek()), 0);
    assert!(env.said.iter().any(|l| l.starts_with("[5/5] done")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn without_an_accepted_licence_nothing_is_fetched_or_installed() {
    let d = tmp("nolic");
    let mut env = Fake::new();
    let e = go(&mut env, &d).unwrap_err();
    assert!(e.contains("licence was not accepted") && e.contains("Nothing was downloaded or installed"), "{e}");
    assert!(env.fetched.is_empty() && env.installed.is_empty());
    assert!(read_outcome(&d, realtek()).unwrap().headline.starts_with("Not done"));
    // an acceptance of other texts (a new pin) does not count either
    accept_it(&d);
    let mut other = fake_table();
    other.packs[0].docs[0].sha256 = sha256_hex(b"another licence");
    assert!(accepted_in(&other, &d, realtek()).is_none() && accepted_in(&fake_table(), &d, realtek()).is_some());
    assert!(run_with(&mut env, &other, &d, realtek(), false, None).is_err() && env.fetched.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_wrong_hardware_in_memory_data_and_little_space_stop_before_any_download() {
    let d = tmp("stop");
    accept_it(&d);
    let mut env = Fake::new();
    env.present = false;
    assert!(go(&mut env, &d).unwrap_err().contains("no hardware of this pack"));
    // the command line may force it
    let mut env2 = Fake::new();
    env2.present = false;
    assert!(run_with(&mut env2, &fake_table(), &d, realtek(), true, None).is_ok());
    let mut env = Fake::new();
    env.in_memory = true;
    assert!(go(&mut env, &d).unwrap_err().contains("in memory"));
    let mut env = Fake::new();
    env.free = Some(1 << 20);
    assert!(go(&mut env, &d).unwrap_err().contains("MB free"));
    assert!(env.fetched.is_empty() && env.installed.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_file_that_does_not_match_its_pinned_hash_installs_nothing_and_is_deleted() {
    let d = tmp("bad");
    accept_it(&d);
    let mut env = Fake::new();
    env.net[1].1[10] ^= 1; // one flipped bit in the second file
    let e = go(&mut env, &d).unwrap_err();
    assert!(e.contains("rtl8125b-2.fw") && e.contains("not the expected") && e.contains("Nothing was installed"), "{e}");
    assert!(env.installed.is_empty(), "nothing is installed after one bad file");
    assert!(!download_dir(&d, realtek()).join("rtl_nic/rtl8125b-2.fw").exists(), "the bad file is deleted");
    // a short file too
    let mut env = Fake::new();
    env.net[0].1.truncate(100);
    assert!(go(&mut env, &d).unwrap_err().contains("100 bytes, not the expected 3000"));
    assert!(env.installed.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_reboot_is_asked_for_only_when_the_driver_cannot_pick_the_files_up() {
    let d = tmp("reboot");
    accept_it(&d);
    // built into the kernel
    let mut env = Fake::new();
    env.mods = vec![("r8169".into(), ModState::Builtin)];
    let o = go(&mut env, &d).unwrap();
    assert!(o.reboot_needed && o.headline.contains("Reboot to finish") && o.lines.iter().any(|l| l.contains("built into the kernel")), "{o:?}");
    // the driver did not load again
    let mut env = Fake::new();
    env.report = vec!["driver r8169 did not load: no such module".into()];
    assert!(go(&mut env, &d).unwrap().reboot_needed);
    // the device has no driver bound
    let mut env = Fake::new();
    env.bound = Some(false);
    let o = go(&mut env, &d).unwrap();
    assert!(o.reboot_needed && o.lines.iter().any(|l| l.contains("no driver bound")));
    // loaded, reloaded, bound: no reboot; a device that cannot be told (None) does not ask either
    let mut env = Fake::new();
    env.bound = None;
    assert!(!go(&mut env, &d).unwrap().reboot_needed);
    // the restart wish ends with the restart: an older result than the boot time is not pending
    let o = Outcome { at: 1000, installed: true, reboot_needed: true, ..Default::default() };
    assert!(reboot_pending(&o, 1500, 600) && !reboot_pending(&o, 1500, 400), "up since 900: the run at 1000 is after the start; up since 1100: restarted since");
    assert!(!reboot_pending(&Outcome { reboot_needed: false, ..o.clone() }, 1500, 600));
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------------------------------------ the offline route

#[test]
fn a_folder_or_stick_with_the_right_files_installs_the_same_way() {
    let d = tmp("folder-data");
    let stick = tmp("stick");
    accept_it(&d);
    // flat names and the repository layout both work; unknown files are ignored
    std::fs::write(stick.join("rtl8168h-2.fw"), &fake_files()[0].1).unwrap();
    std::fs::create_dir_all(stick.join("rtl_nic")).unwrap();
    std::fs::write(stick.join("rtl_nic/rtl8125b-2.fw"), &fake_files()[1].1).unwrap();
    std::fs::write(stick.join("holiday.jpg"), b"x").unwrap();
    std::fs::write(stick.join("LICENSE.r8169"), &fake_files()[2].1).unwrap();
    let mut env = Fake::new();
    let o = run_with(&mut env, &fake_table(), &d, realtek(), false, Some(&stick)).unwrap();
    assert!(o.installed && o.files == 2 && o.source == "folder" && env.fetched.is_empty(), "{o:?}");
    assert_eq!(env.installed[0].1.files.len(), 3, "two files and the licence text");
    // one wrong file refuses the whole stick
    std::fs::write(stick.join("rtl8168h-2.fw"), b"tampered").unwrap();
    let mut env = Fake::new();
    let e = run_with(&mut env, &fake_table(), &d, realtek(), false, Some(&stick)).unwrap_err();
    assert!(e.contains("not the expected") && e.contains("Nothing was installed"), "{e}");
    assert!(env.installed.is_empty());
    // an empty folder, and no acceptance
    let empty = tmp("empty-stick");
    assert!(run_with(&mut Fake::new(), &fake_table(), &d, realtek(), false, Some(&empty)).unwrap_err().contains("none of the 2 files"));
    let d2 = tmp("folder-data2");
    assert!(run_with(&mut Fake::new(), &fake_table(), &d2, realtek(), false, Some(&stick)).unwrap_err().contains("licence was not accepted"));
    for p in [d, stick, empty, d2] {
        let _ = std::fs::remove_dir_all(&p);
    }
}

// ------------------------------------------------------------------------------------------------ the real download

struct Server {
    body: Vec<u8>,
    /// "range" (honours open-ended ranges), "whole" (ignores them), "big" (sends more than asked), "redirect:URL"
    mode: Mutex<String>,
    asked: Mutex<Vec<(String, Option<String>)>>,
}

impl iyakki_proxy::Streamer for Server {
    fn open(&self, url: &str, headers: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        let range = headers.iter().find(|h| h.0 == "Range").map(|h| h.1.clone());
        self.asked.lock().unwrap().push((url.to_string(), range.clone()));
        let mode = self.mode.lock().unwrap().clone();
        if let Some(to) = mode.strip_prefix("redirect:") {
            return Ok(iyakki_proxy::StreamHead { status: 302, headers: vec![("location".into(), to.into())], body: Box::new(std::io::Cursor::new(Vec::new())) });
        }
        let head = |status, headers, b: Vec<u8>| Ok(iyakki_proxy::StreamHead { status, headers, body: Box::new(std::io::Cursor::new(b)) });
        match (mode.as_str(), range) {
            ("range", Some(r)) => {
                let a: usize = r.strip_prefix("bytes=").unwrap().trim_end_matches('-').parse().unwrap();
                head(206, vec![("content-range".into(), format!("bytes {a}-{}/{}", self.body.len() - 1, self.body.len()))], self.body[a..].to_vec())
            }
            ("big", _) => head(200, vec![], [self.body.clone(), vec![0u8; 10]].concat()),
            ("404", _) => head(404, vec![], vec![]),
            _ => head(200, vec![], self.body.clone()),
        }
    }
}

const URL: &str = "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/rtl_nic/rtl8168h-2.fw?id=1a935bfb330d4a46786eaa903580db9e757b84fa";

fn get(s: &Server, url: &str, want: u64, dest: &Path) -> Result<(), String> {
    fetch_resume(s, url, want, dest, 1, Duration::ZERO, &mut |_| {}, &|| false)
}

#[test]
fn a_partial_download_is_resumed_from_the_bytes_on_disk() {
    let d = tmp("resume");
    let body = fake_files()[0].1.clone();
    let s = Server { body: body.clone(), mode: Mutex::new("range".into()), asked: Mutex::new(vec![]) };
    let dest = d.join("a/b.fw");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, &body[..1200]).unwrap();
    get(&s, URL, body.len() as u64, &dest).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert_eq!(s.asked.lock().unwrap()[0].1.as_deref(), Some("bytes=1200-"), "only the rest is asked for");
    // complete already: nothing is asked
    s.asked.lock().unwrap().clear();
    get(&s, URL, body.len() as u64, &dest).unwrap();
    assert!(s.asked.lock().unwrap().is_empty());
    // a server that ignores the range sends the whole file: the file starts over, no doubled bytes
    *s.mode.lock().unwrap() = "whole".into();
    std::fs::write(&dest, &body[..500]).unwrap();
    get(&s, URL, body.len() as u64, &dest).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    // a longer file on disk than expected starts over
    std::fs::write(&dest, vec![1u8; body.len() + 5]).unwrap();
    get(&s, URL, body.len() as u64, &dest).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn foreign_hosts_http_too_much_data_and_missing_files_are_refused() {
    let d = tmp("refuse");
    let body = fake_files()[0].1.clone();
    let s = Server { body: body.clone(), mode: Mutex::new("range".into()), asked: Mutex::new(vec![]) };
    let dest = d.join("x.fw");
    let n = body.len() as u64;
    for bad in ["http://git.kernel.org/x", "https://evil.example.com/x", "https://notgit.kernel.org.evil.com/x", "https://user@git.kernel.org/x"] {
        assert!(get(&s, bad, n, &dest).unwrap_err().contains("not an https address of git.kernel.org"), "{bad}");
    }
    assert!(s.asked.lock().unwrap().is_empty(), "nothing was asked from a refused address");
    *s.mode.lock().unwrap() = "redirect:https://evil.example.com/y".into();
    assert!(get(&s, URL, n, &dest).unwrap_err().contains("not an https address"));
    *s.mode.lock().unwrap() = "big".into();
    assert!(get(&s, URL, n, &dest).unwrap_err().contains("more than the") && !dest.exists());
    *s.mode.lock().unwrap() = "404".into();
    assert!(get(&s, URL, n, &dest).unwrap_err().contains("HTTP 404"));
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------------------------------------ install on a real (sandboxed) data folder

#[test]
fn an_installed_pack_is_a_driver_pack_on_the_data_volume_that_a_restart_links_again() {
    let d = tmp("real-install");
    accept_it(&d);
    let st: std::sync::Arc<dyn iyakki_proxy::Streamer> = std::sync::Arc::new(Server { body: vec![], mode: Mutex::new("404".into()), asked: Mutex::new(vec![]) });
    struct Wrapped(RealEnv, Vec<(String, Vec<u8>)>);
    impl Env for Wrapped {
        fn say(&mut self, _l: &str) {}
        fn hardware_present(&self, _d: &Def) -> bool {
            true
        }
        fn free_bytes(&self, _d: &Path) -> Option<u64> {
            Some(1 << 30)
        }
        fn data_in_memory(&self) -> bool {
            false
        }
        fn fetch(&mut self, url: &str, _w: u64, dest: &Path) -> Result<(), String> {
            let path = url.split("/plain/").nth(1).unwrap().split('?').next().unwrap();
            std::fs::write(dest, &self.1.iter().find(|(p, _)| p == path).unwrap().1).map_err(|e| e.to_string())
        }
        fn install(&mut self, def: &Def, v: &str, s: &str, p: &pack::Pack) -> Result<apply::Report, String> {
            self.0.install(def, v, s, p)
        }
        fn module_state(&self, _n: &str) -> ModState {
            ModState::Absent
        }
        fn device_bound(&self, _d: &Def) -> Option<bool> {
            None
        }
        fn now(&self) -> i64 {
            7
        }
    }
    let mut env = Wrapped(RealEnv::new(&d, st), fake_files().into_iter().map(|(p, b)| (p.to_string(), b)).collect());
    let o = run_with(&mut env, &fake_table(), &d, realtek(), false, None).unwrap();
    assert!(o.installed);
    let rec = installed(&d, realtek()).expect("the pack is on the data volume");
    assert_eq!(rec.id, "fw-realtek-net");
    assert_eq!(rec.level, "local");
    assert!(rec.files.contains(&"rtl_nic/rtl8168h-2.fw".to_string()) && rec.license == "LicenseRef-Realtek-r8169");
    // outside a device image the links go to a sandbox folder inside the data folder: a "boot" links them again from the data volume
    let (fs, real) = apply::Fs::for_data(&d);
    if !real {
        let link = fs.fw_root.join("rtl_nic/rtl8168h-2.fw");
        assert!(std::fs::read_link(&link).is_ok(), "{}", link.display());
        let _ = std::fs::remove_file(&link);
        let log = apply::apply_boot(&fs, &iyakki_drivers::run::Dry);
        assert!(log.iter().any(|l| l.contains("fw-realtek-net")), "{log:?}");
        assert!(std::fs::read_link(&link).is_ok(), "linked again at start");
        assert_eq!(std::fs::read(fs.pack_dir("fw-realtek-net").join("files/rtl_nic/rtl8168h-2.fw")).unwrap(), fake_files()[0].1);
    }
    remove(&d, realtek()).unwrap();
    assert!(installed(&d, realtek()).is_none());
    let _ = std::fs::remove_dir_all(&d);
}

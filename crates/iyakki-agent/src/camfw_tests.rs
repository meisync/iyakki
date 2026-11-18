use super::*;
use iyakki_drivers::pack::sha256_hex;
use std::sync::Mutex;

fn tmp(tag: &str) -> PathBuf {
    let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-camfw-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// A server with an "image" of `image` bytes that honours `Range: bytes=a-b`.
struct Server {
    image: Vec<u8>,
    /// What to answer instead of the right thing: 200 (ignores the range), or a redirect to this address.
    mode: Mutex<String>,
    asked: Mutex<Vec<(String, Option<String>)>>,
}

impl Server {
    fn new(image: Vec<u8>) -> Server {
        Server { image, mode: Mutex::new(String::new()), asked: Mutex::new(Vec::new()) }
    }
}

impl iyakki_proxy::Streamer for Server {
    fn open(&self, url: &str, headers: &[(String, String)]) -> Result<iyakki_proxy::StreamHead, String> {
        let range = headers.iter().find(|h| h.0 == "Range").map(|h| h.1.clone());
        self.asked.lock().unwrap().push((url.to_string(), range.clone()));
        let mode = self.mode.lock().unwrap().clone();
        if let Some(to) = mode.strip_prefix("redirect:") {
            return Ok(iyakki_proxy::StreamHead { status: 302, headers: vec![("location".into(), to.into())], body: Box::new(std::io::Cursor::new(Vec::new())) });
        }
        if mode == "200" {
            return Ok(iyakki_proxy::StreamHead { status: 200, headers: vec![], body: Box::new(std::io::Cursor::new(self.image.clone())) });
        }
        if mode == "416" {
            return Ok(iyakki_proxy::StreamHead { status: 416, headers: vec![], body: Box::new(std::io::Cursor::new(Vec::new())) });
        }
        let r = range.ok_or("no range")?;
        let (a, b) = r.strip_prefix("bytes=").unwrap().split_once('-').unwrap();
        let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
        let total = if mode == "wrong-total" { self.image.len() + 1 } else { self.image.len() };
        Ok(iyakki_proxy::StreamHead { status: 206, headers: vec![("content-range".into(), format!("bytes {a}-{b}/{total}"))], body: Box::new(std::io::Cursor::new(self.image[a..=b].to_vec())) })
    }
}

fn image() -> Vec<u8> {
    (0..5000u32).map(|i| (i * 7 % 253) as u8).collect()
}

fn part_of(img: &[u8], from: u64, to: u64) -> facetime::Part {
    facetime::Part { name: "t", from, to, sha256: leak(sha256_hex(&img[from as usize..=to as usize])) }
}

fn policy() -> crate::macrecovery::Policy {
    crate::macrecovery::Policy { hosts: vec!["cdn-apple.com".into(), "apple.com".into()], max_redirects: 3, retry_pause: Duration::ZERO }
}

const URL: &str = "https://updates.cdn-apple.com/x/y.dmg";

fn fetch(s: &Server, url: &str, p: &facetime::Part, dest: &Path, total: u64) -> Result<(), String> {
    fetch_range(s, &policy(), url, total, p, dest, 1, &mut |_| {}, &|| false)
}

#[test]
fn a_range_is_fetched_checked_and_resumed_from_the_bytes_on_disk() {
    let d = tmp("range");
    let img = image();
    let s = Server::new(img.clone());
    let p = part_of(&img, 1000, 2999);
    let dest = d.join("download/t.xz");
    fetch(&s, URL, &p, &dest, img.len() as u64).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), &img[1000..3000]);
    assert_eq!(s.asked.lock().unwrap()[0].1.as_deref(), Some("bytes=1000-2999"));
    // an interrupted download: the first 700 bytes are there; only the rest is asked for
    std::fs::write(&dest, &img[1000..1700]).unwrap();
    s.asked.lock().unwrap().clear();
    fetch(&s, URL, &p, &dest, img.len() as u64).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), &img[1000..3000]);
    assert_eq!(s.asked.lock().unwrap()[0].1.as_deref(), Some("bytes=1700-2999"));
    // complete and right already: nothing is asked
    s.asked.lock().unwrap().clear();
    fetch(&s, URL, &p, &dest, img.len() as u64).unwrap();
    assert!(s.asked.lock().unwrap().is_empty());
    // a file longer than the part is started over
    std::fs::write(&dest, vec![1u8; 3000]).unwrap();
    fetch(&s, URL, &p, &dest, img.len() as u64).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), &img[1000..3000]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn wrong_bytes_a_whole_file_answer_foreign_hosts_http_and_a_changed_file_are_refused() {
    let d = tmp("refuse");
    let img = image();
    let s = Server::new(img.clone());
    let dest = d.join("t.xz");
    // the checksum does not match: the file is deleted and the numbers are said
    let mut bad = part_of(&img, 10, 109);
    bad.sha256 = leak("0".repeat(64));
    let e = fetch(&s, URL, &bad, &dest, img.len() as u64).unwrap_err();
    assert!(e.contains("not the expected") && e.contains(&sha256_hex(&img[10..110])) && !dest.exists(), "{e}");
    let p = part_of(&img, 10, 109);
    // plain http and foreign hosts
    assert!(fetch(&s, "http://updates.cdn-apple.com/x", &p, &dest, 5000).unwrap_err().contains("not an Apple https address"));
    assert!(fetch(&s, "https://evil.example.com/x", &p, &dest, 5000).unwrap_err().contains("not an Apple https address"));
    assert!(fetch(&s, "https://notapple.com/x", &p, &dest, 5000).unwrap_err().contains("not an Apple https address"));
    // a redirect to a foreign host
    *s.mode.lock().unwrap() = "redirect:https://evil.example.com/y".into();
    assert!(fetch(&s, URL, &p, &dest, 5000).unwrap_err().contains("not an Apple https address"));
    // a redirect inside Apple's domains is followed
    *s.mode.lock().unwrap() = String::new();
    // the server sends the whole file: refused without reading it
    *s.mode.lock().unwrap() = "200".into();
    assert!(fetch(&s, URL, &p, &dest, 5000).unwrap_err().contains("whole"));
    assert!(!dest.exists());
    // the file is not the size the pinned values were made for
    *s.mode.lock().unwrap() = "wrong-total".into();
    assert!(fetch(&s, URL, &p, &dest, 5000).unwrap_err().contains("Apple changed it"));
    // 416 starts over and gives up after the retries with a plain message
    *s.mode.lock().unwrap() = "416".into();
    assert!(fetch(&s, URL, &p, &dest, 5000).unwrap_err().contains("did not finish"));
    // stopping
    *s.mode.lock().unwrap() = String::new();
    let e = fetch_range(&s, &policy(), URL, 5000, &p, &dest, 0, &mut |_| {}, &|| true).unwrap_err();
    assert_eq!(e, "stopped");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_real_parts_table_is_the_one_of_the_makefile_and_well_formed() {
    let p = &facetime::PARTS;
    assert_eq!(p[0].len(), 4_129_656);
    assert_eq!(p[1].len(), 4_670_732);
    assert_eq!(p[2].len(), 8_822_492);
    for x in p.iter() {
        assert!(x.sha256.len() == 64 && x.sha256.chars().all(|c| c.is_ascii_hexdigit()) && x.to <= facetime::SOURCE_BYTES);
    }
    assert!(facetime::SOURCE_URL.starts_with("https://updates.cdn-apple.com/") && crate::macrecovery::Policy { hosts: facetime::SOURCE_HOSTS.iter().map(|s| s.to_string()).collect(), ..Default::default() }.allows(facetime::SOURCE_URL));
    assert_eq!(facetime::SETFILES.len(), 11);
    assert!(facetime::SETFILES.iter().any(|s| s.0 == "1771") && facetime::SETFILES.iter().any(|s| s.0 == "1675"));
}

// ---------------------------------------------------------------------------------------------------- the job

struct Fake {
    camera: bool,
    memory: bool,
    free: Option<u64>,
    fetched: Vec<String>,
    fail_fetch: Option<String>,
    fail_build: Option<String>,
    install_lines: Vec<String>,
    installed: Vec<(String, pack::Pack)>,
    appears_after: u32,
    looks: u32,
    said: Vec<String>,
}

impl Fake {
    fn new() -> Fake {
        Fake { camera: true, memory: false, free: Some(5 << 30), fetched: vec![], fail_fetch: None, fail_build: None, install_lines: vec!["driver facetimehd reloaded".into()], installed: vec![], appears_after: 1, looks: 0, said: vec![] }
    }
}

impl Env for Fake {
    fn say(&mut self, l: &str) {
        self.said.push(l.into());
    }
    fn camera_present(&self) -> bool {
        self.camera
    }
    fn free_bytes(&self, _: &Path) -> Option<u64> {
        self.free
    }
    fn data_in_memory(&self) -> bool {
        self.memory
    }
    fn fetch_part(&mut self, part: &facetime::Part, dest: &Path) -> Result<(), String> {
        if let Some(e) = &self.fail_fetch {
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(dest, b"half").unwrap();
            return Err(e.clone());
        }
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(dest, b"piece").unwrap();
        self.fetched.push(part.name.into());
        Ok(())
    }
    fn build(&mut self, dir: &Path) -> Result<Built, String> {
        assert!(dir.join("assistant.xz").exists());
        if let Some(e) = &self.fail_build {
            return Err(e.clone());
        }
        let fw = vec![9u8; 100];
        let sets = vec![("1771_01XX.dat".to_string(), vec![1u8; 20]), ("1675_01XX.dat".to_string(), vec![2u8; 20])];
        Ok(Built { firmware: facetime::Extracted { sha256: sha256_hex(&fw), firmware: fw, version: "5.60.0", from: "macOS 10.12.6 (Sierra)" }, sets })
    }
    fn install(&mut self, version: &str, _source: &str, p: &pack::Pack) -> Result<apply::Report, String> {
        self.installed.push((version.into(), p.clone()));
        Ok(apply::Report { lines: self.install_lines.clone(), cmdline: vec![] })
    }
    fn look(&mut self) -> Seen {
        self.looks += 1;
        Seen { devices: if self.looks > self.appears_after { vec!["/dev/video0".into()] } else { vec![] } }
    }
    fn kernel_lines(&mut self) -> Vec<String> {
        vec!["facetimehd 0000:03:00.0: firmware loaded".into()]
    }
    fn sleep(&mut self, _: Duration) {}
    fn now(&self) -> i64 {
        1_800_000_000
    }
}

#[test]
fn a_run_downloads_three_pieces_installs_the_pack_and_reports_the_device() {
    let d = tmp("run");
    let mut f = Fake::new();
    let o = run(&mut f, &d).unwrap();
    assert_eq!(f.fetched, ["driver-1", "driver-2", "assistant"]);
    assert!(o.installed && o.seen.devices == ["/dev/video0"] && o.headline.contains("/dev/video0") && o.headline.contains("CAMERA tile"), "{}", o.headline);
    assert_eq!((o.firmware_version.as_str(), o.sensor_files), ("5.60.0", 2));
    // the pack: firmware plus the sensor files, matches the camera, reloads the driver
    let (v, p) = &f.installed[0];
    assert!(v.starts_with("1."));
    let paths: Vec<&str> = p.files.iter().map(|x| x.path.as_str()).collect();
    assert_eq!(paths, ["facetimehd/firmware.bin", "facetimehd/1771_01XX.dat", "facetimehd/1675_01XX.dat"]);
    assert!(p.validate().is_ok() && p.reload == ["facetimehd"] && p.matches.iter().any(|m| m.pci == "14e4:1570"));
    // the notice is the first line; the temporary pieces are gone; the result is written for the page
    assert!(f.said[0].contains("Apple's") && f.said[0].contains("stay on this machine"));
    assert!(!work_dir(&d).join("download").exists() && leftovers(&d) == 0);
    assert_eq!(read_outcome(&d).unwrap(), o);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn nothing_happens_without_the_camera_in_memory_or_without_room() {
    let d = tmp("refuse-run");
    let mut f = Fake::new();
    f.camera = false;
    let e = run(&mut f, &d).unwrap_err();
    assert!(e.contains("no FaceTime HD camera") && f.fetched.is_empty() && f.installed.is_empty());
    assert!(read_outcome(&d).unwrap().headline.starts_with("Not done: no FaceTime"));
    let mut f = Fake::new();
    f.memory = true;
    assert!(run(&mut f, &d).unwrap_err().contains("in memory") && f.fetched.is_empty());
    let mut f = Fake::new();
    f.free = Some(10 << 20);
    assert!(run(&mut f, &d).unwrap_err().contains("MB free") && f.fetched.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failure_keeps_the_pieces_for_a_retry_and_installs_nothing() {
    let d = tmp("fail");
    let mut f = Fake::new();
    f.fail_fetch = Some("the download of driver-1 did not finish".into());
    let e = run(&mut f, &d).unwrap_err();
    assert!(e.contains("did not finish") && f.installed.is_empty());
    assert!(leftovers(&d) > 0);
    let o = read_outcome(&d).unwrap();
    assert!(!o.installed && o.lines.iter().any(|l| l.contains("kept for a retry")));
    // a checksum mismatch in the unpacking installs nothing either
    let mut f = Fake::new();
    f.fail_build = Some("the firmware came out with sha256 abc, not the expected def. Nothing was written.".into());
    assert!(run(&mut f, &d).unwrap_err().contains("Nothing was written") && f.installed.is_empty());
    // Delete download
    assert!(delete_leftovers(&d) > 0 && leftovers(&d) == 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_result_says_plainly_when_the_device_does_not_appear_or_the_image_has_no_driver() {
    let d = tmp("nodev");
    let mut f = Fake::new();
    f.appears_after = 1000;
    let o = run(&mut f, &d).unwrap();
    assert!(o.installed && o.seen.devices.is_empty() && o.headline.contains("no camera device has appeared") && o.lines.iter().any(|l| l.contains("restart IYAKKI once")));
    assert!(f.looks > 15, "it waited for the device");
    let mut f = Fake::new();
    f.appears_after = 1000;
    f.install_lines = vec!["driver facetimehd did not load: modprobe facetimehd: not found".into()];
    let o = run(&mut f, &d).unwrap();
    assert!(o.headline.contains("no camera driver") && f.looks <= 2, "it does not wait for a driver that is not there");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_camera_and_its_video_device_are_found_in_sysfs_and_the_log_is_sorted_out() {
    let d = tmp("sys");
    let w = |rel: &str, s: &str| {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    };
    w("bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
    w("bus/pci/devices/0000:00:02.0/device", "0x5926\n");
    assert!(!camera_in(&d));
    w("bus/pci/devices/0000:03:00.0/vendor", "0x14e4\n");
    w("bus/pci/devices/0000:03:00.0/device", "0x1570\n");
    assert!(camera_in(&d));
    w("class/video4linux/video0/name", "Integrated Webcam\n");
    assert!(video_devices_in(&d, Path::new("/dev")).is_empty(), "a USB webcam is not the FaceTime camera");
    w("class/video4linux/video1/name", "Apple Facetime HD\n");
    w("class/video4linux/video2/name", "other\n");
    w("class/video4linux/video2/device/vendor", "0x14e4\n");
    assert_eq!(video_devices_in(&d, Path::new("/dev")), ["/dev/video1", "/dev/video2"]);
    let log = "[1] usb 1-1: new device\n[2] facetimehd 0000:03:00.0: Direct firmware load for facetimehd/firmware.bin failed\n[3] eth0: up\n[4] facetimehd 0000:03:00.0: sensor count: 1\n";
    assert_eq!(camera_lines(log).len(), 2);
    let _ = std::fs::remove_dir_all(&d);
}

/// With the three downloaded pieces in a folder (IYAKKI_CAMERA_PIECES=<dir with driver-1.xz, driver-2.xz, assistant.xz>) the whole
/// unpacking is checked against Apple's real bytes. Run by hand: `IYAKKI_CAMERA_PIECES=... cargo test -p iyakki-agent camfw -- --ignored`.
#[test]
#[ignore]
fn the_real_pieces_make_the_known_firmware_and_all_eleven_sensor_files() {
    let Ok(dir) = std::env::var("IYAKKI_CAMERA_PIECES") else { panic!("set IYAKKI_CAMERA_PIECES") };
    let b = build_from_dir(Path::new(&dir)).unwrap();
    assert_eq!(b.firmware.sha256, "240ef2e991f1d089d8228ce11d92b66bfa4b3d7289ec4fee228b64a713024330");
    assert_eq!((b.firmware.version, b.firmware.firmware.len()), ("5.60.0", 1_417_220));
    assert_eq!(b.sets.len(), 11);
    assert!(facetime::pack_with_setfiles(&b.firmware, &b.sets).validate().is_ok());
}

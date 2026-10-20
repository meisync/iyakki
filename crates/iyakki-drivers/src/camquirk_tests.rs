use super::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

/// An in-memory sysfs plus a fake kernel (uvcvideo, snd-usb-audio, modprobe). Every sysfs write, module load and wait is
/// recorded in one list, in order, so the tests can check the sequence and the time spent.
#[derive(Default)]
struct Fake {
    files: RefCell<BTreeMap<String, String>>,
    events: RefCell<Vec<String>>,
    klog: RefCell<String>,
    /// device -> the quirks value its probe needs (absent: the default works; "never": nothing works)
    needs: RefCell<BTreeMap<String, String>>,
    /// devices that no longer answer
    wedged: RefCell<BTreeSet<String>>,
    /// devices that stop answering once snd-usb-audio talks to their microphone
    mic_wedges: RefCell<BTreeSet<String>>,
}

const DEFAULT_QUIRKS: &str = "4294967295";

impl Fake {
    fn new() -> Fake {
        Fake::default()
    }
    /// uvcvideo and snd-usb-audio already loaded (a hot plug after boot).
    fn loaded() -> Fake {
        let f = Fake::new();
        f.load_uvc();
        f.put(&format!("{SND_USB_AUDIO_MODULE}/refcnt"), "1\n");
        f
    }
    fn load_uvc(&self) {
        self.put(UVC_QUIRKS_PARAM, &format!("{DEFAULT_QUIRKS}\n"));
        self.put(&format!("{UVC_DRIVER}/bind"), "");
        self.put(&format!("{UVC_DRIVER}/unbind"), "");
    }
    fn put(&self, rel: &str, v: &str) {
        self.files.borrow_mut().insert(rel.to_string(), v.to_string());
    }
    fn remove(&self, rel: &str) {
        self.files.borrow_mut().retain(|k, _| k != rel && !k.starts_with(&format!("{rel}/")));
    }
    fn events(&self) -> Vec<String> {
        self.events.borrow().clone()
    }
    /// The events without the waits.
    fn writes(&self) -> Vec<String> {
        self.events().into_iter().filter(|e| !e.starts_with("wait")).collect()
    }
    fn quirks(&self) -> String {
        self.read(UVC_QUIRKS_PARAM).unwrap().trim().to_string()
    }
    fn klog(&self) -> String {
        self.klog.borrow().clone()
    }
    fn log(&self, line: &str) {
        self.klog.borrow_mut().push_str(&format!("{line}\n"));
    }
    fn waited(&self) -> Duration {
        self.events().iter().filter_map(|e| e.strip_prefix("wait ")).map(|ms| Duration::from_millis(ms.parse().unwrap())).sum()
    }
    fn wait(&self) -> impl FnMut(Duration) + '_ {
        move |d| self.events.borrow_mut().push(format!("wait {}", d.as_millis()))
    }
    /// Plug a USB device in: the kernel logs it, creates its interfaces and lets loaded drivers probe them (no quirk).
    fn plug(&self, dev: &str, vid: u16, pid: u16, ifaces: &[(u8, u8, u8)]) {
        self.log(&format!("[ 2.0] usb {dev}: new high-speed USB device number 5 using xhci_hcd"));
        self.put(&format!("{USB_DEVICES}/{dev}/idVendor"), &format!("{vid:04x}\n"));
        self.put(&format!("{USB_DEVICES}/{dev}/idProduct"), &format!("{pid:04x}\n"));
        self.put(&format!("{USB_DEVICES}/{dev}/bNumInterfaces"), &format!("{:>2}\n", ifaces.len()));
        self.put(&format!("{USB_DEVICES}/{dev}/power/control"), "auto\n");
        for (n, c, s) in ifaces {
            self.interface(dev, *n, *c, *s);
        }
        self.drivers_probe();
    }
    fn interface(&self, dev: &str, n: u8, class: u8, sub: u8) {
        let i = format!("{USB_DEVICES}/{dev}:1.{n}");
        self.put(&format!("{i}/bInterfaceClass"), &format!("{class:02x}\n"));
        self.put(&format!("{i}/bInterfaceSubClass"), &format!("{sub:02x}\n"));
        self.put(&format!("{i}/authorized"), "1\n");
    }
    fn all_interfaces(&self) -> Vec<Interface> {
        usb_devices(self).iter().flat_map(|d| interfaces(self, d)).collect()
    }
    /// What the kernel does when a driver is (newly) there: bind every authorized interface it handles.
    fn drivers_probe(&self) {
        for i in self.all_interfaces() {
            if !authorized(self, &i.name) || self.exists(&format!("{USB_DEVICES}/{}/driver", i.name)) {
                continue;
            }
            if i.is_video_control() && uvcvideo_loaded(self) {
                let _ = self.uvc_probe(&i.name);
            }
            if i.is_audio() && self.exists(SND_USB_AUDIO_MODULE) {
                let dev = i.name.split(':').next().unwrap().to_string();
                self.put(&format!("{USB_DEVICES}/{}/driver", i.name), "snd-usb-audio");
                self.events.borrow_mut().push(format!("snd-usb-audio talks to {}", i.name));
                if self.mic_wedges.borrow().contains(&dev) {
                    self.log(&format!("[ 2.1] usb {dev}: 3:1: cannot get freq at ep 0x89"));
                    self.wedged.borrow_mut().insert(dev);
                }
            }
        }
    }
    /// uvcvideo's probe of a video control interface with the quirks parameter as it is now.
    fn uvc_probe(&self, iface: &str) -> Result<(), String> {
        let dev = iface.split(':').next().unwrap().to_string();
        let stream = iface.replace(":1.0", ":1.1");
        if !authorized(self, iface) || !authorized(self, &stream) {
            return Err("No such device".into());
        }
        self.events.borrow_mut().push(format!("uvcvideo probes {iface} with quirks {}", self.quirks()));
        if self.wedged.borrow().contains(&dev) {
            self.log(&format!("[ 9.0] uvcvideo {stream}: Failed to query (129) UVC probe control : -110 (exp. 26)."));
            return Err("No such device".into());
        }
        let need = self.needs.borrow().get(&dev).cloned().unwrap_or_else(|| DEFAULT_QUIRKS.to_string());
        if self.quirks() != need {
            self.log(&format!("[ 3.7] uvcvideo {stream}: Failed to query (129) UVC probe control : -32 (exp. 26)."));
            return Err("No such device".into());
        }
        self.put(&format!("{USB_DEVICES}/{iface}/driver"), "uvcvideo");
        self.put(&format!("{USB_DEVICES}/{iface}/video4linux/video0/name"), "cam");
        self.put(&format!("{USB_DEVICES}/{iface}/video4linux/video1/name"), "cam");
        Ok(())
    }
}

impl Sysfs for Fake {
    fn read(&self, rel: &str) -> Option<String> {
        self.files.borrow().get(rel).cloned()
    }
    fn write(&self, rel: &str, value: &str) -> Result<(), String> {
        self.events.borrow_mut().push(format!("{rel}={value}"));
        if !self.exists(rel) {
            return Err(format!("{rel}: no such file"));
        }
        if rel == format!("{UVC_DRIVER}/bind") {
            if self.exists(&format!("{USB_DEVICES}/{value}/driver")) {
                return Err("Device or resource busy".into());
            }
            return self.uvc_probe(value).map_err(|e| format!("{rel}: {e} (os error 19)"));
        }
        if rel == format!("{UVC_DRIVER}/unbind") {
            self.remove(&format!("{USB_DEVICES}/{value}/driver"));
            self.remove(&format!("{USB_DEVICES}/{value}/video4linux"));
            return Ok(());
        }
        if let Some(i) = rel.strip_suffix("/authorized") {
            if value == "0" {
                self.remove(&format!("{i}/driver"));
                self.remove(&format!("{i}/video4linux"));
            }
        }
        self.put(rel, value);
        Ok(())
    }
    fn list(&self, rel: &str) -> Vec<String> {
        let prefix = format!("{rel}/");
        let mut v: Vec<String> = self.files.borrow().keys().filter_map(|k| k.strip_prefix(&prefix)).map(|r| r.split('/').next().unwrap_or("").to_string()).collect();
        v.dedup();
        v
    }
    fn exists(&self, rel: &str) -> bool {
        let prefix = format!("{rel}/");
        self.files.borrow().keys().any(|k| k == rel || k.starts_with(&prefix))
    }
}

impl Runner for Fake {
    fn run(&self, program: &str, args: &[&str]) -> Result<(), String> {
        self.events.borrow_mut().push(format!("{program} {}", args.join(" ")));
        match args {
            ["uvcvideo"] => self.load_uvc(),
            ["snd-usb-audio"] => self.put(&format!("{SND_USB_AUDIO_MODULE}/refcnt"), "0\n"),
            _ => return Err(format!("unexpected {program} {args:?}")),
        }
        self.drivers_probe();
        Ok(())
    }
}

/// The Fullhan webcam as it shows up: video control, video streaming, audio control, audio streaming.
const FULLHAN: &[(u8, u8, u8)] = &[(0, 0x0e, 1), (1, 0x0e, 2), (2, 0x01, 1), (3, 0x01, 2)];

fn fullhan(f: &Fake, dev: &str) {
    f.needs.borrow_mut().insert(dev.into(), "0x100".into());
    f.plug(dev, 0x1d6c, 0x0103, FULLHAN);
}

fn handle(f: &Fake, dev: &str) -> Vec<String> {
    handle_device(f, f, dev, &|| f.klog(), &mut f.wait())
}

#[test]
fn the_table_knows_the_fullhan_webcam_and_nothing_twice() {
    let q = known(0x1d6c, 0x0103).expect("1d6c:0103 is in the table");
    assert_eq!(q.uvc_quirks, UVC_QUIRK_PROBE_DEF);
    assert_eq!(q.uvc_quirks, 0x100);
    assert!(q.audio_off);
    assert!(q.note.contains("precaution") && q.note.contains("not proven"), "the microphone part is marked as a precaution");
    assert!(known(0x1d6c, 0x0104).is_none());
    assert!(known(0x05ac, 0x8514).is_none());
    for (i, a) in CAMERA_QUIRKS.iter().enumerate() {
        assert!(a.uvc_quirks != 0 && !a.name.is_empty());
        assert!(CAMERA_QUIRKS[i + 1..].iter().all(|b| (a.vendor, a.product) != (b.vendor, b.product)), "one entry per camera");
    }
    assert_eq!(FALLBACK_QUIRKS, [0x100, 0x2]);
    assert_eq!(id_text((0x1d6c, 0x0103)), "1d6c:0103");
}

#[test]
fn the_modprobe_file_keeps_the_hotplug_helper_from_loading_the_two_drivers() {
    let c = modprobe_conf();
    assert!(c.lines().any(|l| l == "blacklist uvcvideo"));
    assert!(c.lines().any(|l| l == "blacklist snd_usb_audio"));
    assert!(!c.contains("options"), "no global quirk: other cameras keep the driver's own defaults");
    let d = std::env::temp_dir().join(format!("iyakki-camquirk-conf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    block_autoload(&d.join("etc/modprobe.d")).unwrap();
    assert_eq!(std::fs::read_to_string(d.join("etc/modprobe.d/iyakki-camera.conf")).unwrap(), c);
    let _ = std::fs::remove_dir_all(&d);
}

/// The Mac mini boot that failed, replayed with the new order: the camera is there before any driver; the hotplug
/// helper loads nothing (blacklisted); IYAKKI loads the drivers itself.
#[test]
fn at_boot_the_first_probe_of_a_known_camera_already_has_the_quirk_and_the_microphone_is_never_touched() {
    let f = Fake::new();
    fullhan(&f, "1-8");
    f.mic_wedges.borrow_mut().insert("1-8".into());
    f.plug("1-4", 0x0c45, 0x6366, &[(0, 0x0e, 1), (1, 0x0e, 2), (2, 0x01, 1)]); // an ordinary camera with a mic
    f.events.borrow_mut().clear();
    let lines = prepare_boot(&f, &f);
    assert_eq!(
        f.writes(),
        vec![
            "bus/usb/devices/1-8/power/control=on",
            "bus/usb/devices/1-8:1.2/authorized=0",
            "bus/usb/devices/1-8:1.3/authorized=0",
            "bus/usb/devices/1-8:1.0/authorized=0",
            "bus/usb/devices/1-8:1.1/authorized=0",
            "modprobe uvcvideo",
            "uvcvideo probes 1-4:1.0 with quirks 4294967295",
            "modprobe snd-usb-audio",
            "snd-usb-audio talks to 1-4:1.2",
        ],
        "known camera held back before uvcvideo loads; its microphone off before snd-usb-audio loads"
    );
    assert_eq!(lines, vec!["camera: 1d6c:0103 (SHENZHEN Fullhan HD 4MP WEBCAM) microphone kept off as a precaution (1-8:1.2, 1-8:1.3)"]);
    assert!(f.wedged.borrow().is_empty(), "the microphone query never happened");
    assert_eq!(video_nodes(&f, "1-4"), vec!["video0", "video1"], "the other camera came up with the driver's defaults");
    // the background thread then binds the held-back camera with the quirk, without waiting first
    f.events.borrow_mut().clear();
    let lines = handle(&f, "1-8");
    assert_eq!(
        f.events(),
        vec![
            "module/uvcvideo/parameters/quirks=0x100",
            "bus/usb/devices/1-8:1.0/authorized=1",
            "bus/usb/devices/1-8:1.1/authorized=1",
            "bus/usb/drivers/uvcvideo/bind=1-8:1.0",
            "uvcvideo probes 1-8:1.0 with quirks 0x100",
            "module/uvcvideo/parameters/quirks=4294967295",
        ],
        "first probe with the quirk, the parameter back to the kernel's default, no wait at all"
    );
    assert_eq!(lines, vec!["camera: 1d6c:0103 needed uvcvideo quirk 0x100 (video0, video1)"]);
    assert!(!f.klog().contains("UVC probe control"), "no failed probe at all");
    assert_eq!(f.quirks(), DEFAULT_QUIRKS);
}

#[test]
fn a_hot_plugged_known_camera_is_rebound_with_the_quirk_at_once_after_the_stock_probe_failed() {
    let f = Fake::loaded();
    fullhan(&f, "1-8");
    assert!(f.klog().contains("UVC probe control : -32"), "the stock probe failed as on the Mac");
    f.events.borrow_mut().clear();
    let lines = handle(&f, "1-8");
    let ev = f.events();
    let bind = ev.iter().position(|e| e == "bus/usb/drivers/uvcvideo/bind=1-8:1.0").expect("bound");
    assert!(ev[..bind].iter().all(|e| !e.starts_with("wait")), "the bind comes before any wait: {ev:?}");
    assert!(ev[..bind].contains(&"module/uvcvideo/parameters/quirks=0x100".to_string()));
    assert!(ev[..bind].contains(&"bus/usb/devices/1-8:1.2/authorized=0".to_string()));
    assert_eq!(f.waited(), Duration::ZERO);
    assert_eq!(lines.last().unwrap(), "camera: 1d6c:0103 needed uvcvideo quirk 0x100 (video0, video1)");
    assert_eq!(f.quirks(), DEFAULT_QUIRKS);
}

#[test]
fn a_hot_plugged_known_camera_when_uvcvideo_was_never_loaded_is_held_back_then_bound() {
    let f = Fake::new();
    f.put(&format!("{SND_USB_AUDIO_MODULE}/refcnt"), "0\n");
    fullhan(&f, "2-1");
    f.events.borrow_mut().clear();
    let lines = handle(&f, "2-1");
    let ev = f.writes();
    let pos = |s: &str| ev.iter().position(|e| e == s).unwrap_or_else(|| panic!("{s} missing from {ev:?}"));
    assert!(pos("bus/usb/devices/2-1:1.0/authorized=0") < pos("modprobe uvcvideo"));
    assert!(pos("modprobe uvcvideo") < pos("module/uvcvideo/parameters/quirks=0x100"));
    assert!(!ev.iter().any(|e| e.contains("with quirks 4294967295")), "no stock probe of the known camera: {ev:?}");
    assert_eq!(lines.last().unwrap(), "camera: 1d6c:0103 needed uvcvideo quirk 0x100 (video0, video1)");
}

#[test]
fn a_camera_that_stopped_answering_is_reported_quickly_instead_of_retried_for_minutes() {
    // the Mac mini case: the microphone query (or something else) wedged it before IYAKKI acted
    let f = Fake::loaded();
    f.mic_wedges.borrow_mut().insert("1-8".into());
    fullhan(&f, "1-8");
    f.events.borrow_mut().clear();
    let lines = handle(&f, "1-8");
    let binds = f.events().iter().filter(|e| e.contains("uvcvideo/bind=")).count();
    assert_eq!(binds, 1, "one bind, then the -110 stops it; no 0x2 retry: {:?}", f.events());
    assert!(f.waited() <= Duration::from_secs(1), "no long waits: {:?}", f.waited());
    assert!(lines.iter().any(|l| l.contains("binding with uvcvideo quirk 0x100 failed") && l.contains("os error 19")), "{lines:?}");
    assert_eq!(lines.last().unwrap(), "camera: 1d6c:0103 stopped answering (UVC probe control timed out, -110); unplug it and plug it in again");
    assert_eq!(f.quirks(), DEFAULT_QUIRKS);
    // replugged: the old -110 from before the replug does not count
    f.wedged.borrow_mut().clear();
    f.mic_wedges.borrow_mut().clear();
    f.remove("bus/usb/devices/1-8:1.0/driver");
    f.log("[ 200.0] usb 1-8: USB disconnect, device number 5");
    fullhan(&f, "1-8");
    assert!(handle(&f, "1-8").last().unwrap().contains("needed uvcvideo quirk 0x100"));
}

#[test]
fn a_failed_bind_of_a_known_camera_is_retried_once_after_half_a_second() {
    let f = Fake::loaded();
    fullhan(&f, "1-8");
    // the first bind fails as if the camera were still busy, the second works
    f.needs.borrow_mut().insert("1-8".into(), "never-first".into());
    f.events.borrow_mut().clear();
    let lines = {
        let mut n = 0;
        handle_device(&f, &f, "1-8", &|| f.klog(), &mut |d| {
            f.events.borrow_mut().push(format!("wait {}", d.as_millis()));
            n += 1;
            if n == 1 {
                f.needs.borrow_mut().insert("1-8".into(), "0x100".into());
            }
        })
    };
    let ev = f.events();
    let first_wait = ev.iter().position(|e| e.starts_with("wait")).unwrap();
    assert_eq!(ev[first_wait], "wait 500");
    assert_eq!(ev.iter().filter(|e| e.contains("uvcvideo/bind=")).count(), 2);
    assert_eq!(f.waited(), RETRY_PAUSE);
    assert_eq!(lines.last().unwrap(), "camera: 1d6c:0103 needed uvcvideo quirk 0x100 (video0, video1)");
}

#[test]
fn the_quirks_parameter_is_restored_even_when_binding_fails() {
    let f = Fake::loaded();
    f.plug("1-8", 0x1d6c, 0x0103, FULLHAN);
    f.needs.borrow_mut().insert("1-8".into(), "never".into());
    let e = bind_with_quirks(&f, &f, "1-8:1.0", 0x100).unwrap_err();
    assert!(e.contains("No such device"), "{e}");
    assert_eq!(f.quirks(), DEFAULT_QUIRKS);
    assert_eq!(f.writes().last().unwrap(), "module/uvcvideo/parameters/quirks=4294967295");
    // a whole run where nothing helps (stall, not timeout): every attempt restores, and the last line says what to do
    f.events.borrow_mut().clear();
    let lines = handle(&f, "1-8");
    let quirk_writes: Vec<String> = f.writes().into_iter().filter(|w| w.starts_with(UVC_QUIRKS_PARAM)).collect();
    assert_eq!(
        quirk_writes,
        vec!["module/uvcvideo/parameters/quirks=0x100", "module/uvcvideo/parameters/quirks=4294967295", "module/uvcvideo/parameters/quirks=0x100", "module/uvcvideo/parameters/quirks=4294967295", "module/uvcvideo/parameters/quirks=0x2", "module/uvcvideo/parameters/quirks=4294967295"],
        "the table's quirk twice (one retry), then 0x2"
    );
    assert_eq!(f.quirks(), DEFAULT_QUIRKS);
    assert!(f.waited() <= Duration::from_secs(1), "the failure is in the log, so no settle wait: {:?}", f.waited());
    assert_eq!(lines.last().unwrap(), "camera: 1d6c:0103 still has no video device after uvcvideo quirks 0x100, 0x2; unplug it and plug it in again");
}

#[test]
fn a_bound_interface_is_unbound_first_and_a_working_camera_is_left_alone() {
    let f = Fake::loaded();
    fullhan(&f, "1-8");
    f.put("bus/usb/devices/1-8:1.0/driver", "uvcvideo");
    f.events.borrow_mut().clear();
    assert!(bind_with_quirks(&f, &f, "1-8:1.0", 0x100).is_ok());
    assert_eq!(f.writes(), vec!["module/uvcvideo/parameters/quirks=0x100", "bus/usb/drivers/uvcvideo/unbind=1-8:1.0", "bus/usb/drivers/uvcvideo/bind=1-8:1.0", "uvcvideo probes 1-8:1.0 with quirks 0x100", "module/uvcvideo/parameters/quirks=4294967295"]);
    // an ordinary camera that came up by itself is not touched
    let g = Fake::loaded();
    g.plug("3-2", 0x046d, 0x0825, &[(0, 0x0e, 1), (1, 0x0e, 2), (2, 0x01, 1)]);
    g.events.borrow_mut().clear();
    assert!(handle(&g, "3-2").is_empty());
    assert!(g.events().is_empty());
}

#[test]
fn an_unknown_camera_whose_stock_probe_failed_is_retried_at_once_with_0x100_then_0x2() {
    let f = Fake::loaded();
    f.needs.borrow_mut().insert("2-1.3".into(), "0x2".into());
    f.plug("2-1.3", 0x0c45, 0x6366, &[(0, 0x0e, 1), (1, 0x0e, 2)]);
    f.events.borrow_mut().clear();
    let lines = handle(&f, "2-1.3");
    assert_eq!(
        f.events(),
        vec![
            "module/uvcvideo/parameters/quirks=0x100",
            "bus/usb/drivers/uvcvideo/bind=2-1.3:1.0",
            "uvcvideo probes 2-1.3:1.0 with quirks 0x100",
            "module/uvcvideo/parameters/quirks=4294967295",
            "module/uvcvideo/parameters/quirks=0x2",
            "bus/usb/drivers/uvcvideo/bind=2-1.3:1.0",
            "uvcvideo probes 2-1.3:1.0 with quirks 0x2",
            "module/uvcvideo/parameters/quirks=4294967295",
        ],
        "the -32 is already in the log: no settle wait, and an unknown camera keeps its microphone"
    );
    assert_eq!(
        lines,
        vec![
            "camera: 0c45:6366: retry with uvcvideo quirk 0x100 failed: bus/usb/drivers/uvcvideo/bind: No such device (os error 19)",
            "camera: 0c45:6366 needed uvcvideo quirk 0x2 (video0, video1)",
        ]
    );
    assert_eq!(f.waited(), Duration::ZERO);
}

#[test]
fn without_a_probe_control_failure_no_quirk_is_tried_and_other_devices_are_ignored() {
    let f = Fake::loaded();
    // a camera whose probe neither works nor fails yet (still probing): no node and nothing in the log
    f.plug("1-4", 0x0c45, 0x6366, &[]);
    f.put("bus/usb/devices/1-4/bNumInterfaces", " 2\n");
    f.interface("1-4", 0, 0x0e, 1);
    f.interface("1-4", 1, 0x0e, 2);
    f.log("[ 3.0] uvcvideo 1-4.2:1.1: Failed to query (129) UVC probe control : -32 (exp. 26).");
    f.events.borrow_mut().clear();
    let lines = handle(&f, "1-4");
    assert!(f.writes().is_empty(), "a failure of another device (1-4.2) does not count: {:?}", f.writes());
    assert_eq!(f.waited(), SETTLE);
    assert_eq!(lines, vec!["camera: 0c45:6366 has no video device yet and uvcvideo reported no probe control failure; no quirk tried"]);
    // a keyboard, and something with no ids at all
    f.plug("1-5", 0x05ac, 0x0250, &[(0, 0x03, 1)]);
    f.events.borrow_mut().clear();
    assert!(handle(&f, "1-5").is_empty());
    assert!(handle(&f, "9-9").is_empty());
    assert!(f.events().is_empty());
}

#[test]
fn kernel_log_failures_are_read_per_device_and_only_since_it_was_plugged_in() {
    let log = "[1.0] usb 1-8: new high-speed USB device number 3 using xhci_hcd\n\
               [3.7] uvcvideo 1-8:1.1: Failed to query (129) UVC probe control : -32 (exp. 26).\n\
               [3.7] uvcvideo 1-8:1.1: Failed to initialize the device (-5).\n\
               [120.4] uvcvideo 1-8:1.1: Failed to query (129) UVC probe control : -110 (exp. 26).\n";
    assert_eq!(last_probe_failure(log, "1-8"), Some(-110));
    assert_eq!(last_probe_failure(log, "1-8.1"), None);
    assert_eq!(last_probe_failure("[1] uvcvideo 1-8:1.1: Failed to set UVC probe control : -110\n", "1-8"), Some(-110));
    assert_eq!(last_probe_failure("[1] usb 1-8: can't set config #1, error -110\n", "1-8"), Some(-110));
    assert_eq!(last_probe_failure("[1] usb 1-8: 3:1: cannot get freq at ep 0x89\n", "1-8"), None);
    let replugged = format!("{log}[200.0] usb 1-8: new high-speed USB device number 6 using xhci_hcd\n");
    assert_eq!(last_probe_failure(&replugged, "1-8"), None, "failures before the replug do not count");
    assert!(probe_control_failed(log, "1-8"));
}

#[test]
fn a_just_plugged_camera_is_handled_once_its_interfaces_appear() {
    let f = Fake::loaded();
    f.needs.borrow_mut().insert("1-8".into(), "0x100".into());
    f.plug("1-8", 0x1d6c, 0x0103, &[]);
    f.put("bus/usb/devices/1-8/bNumInterfaces", " 4\n");
    let mut n = 0;
    let lines = handle_device(&f, &f, "1-8", &|| f.klog(), &mut |_| {
        n += 1;
        if n == 3 {
            for (i, c, s) in FULLHAN {
                f.interface("1-8", *i, *c, *s);
            }
        }
    });
    assert!(lines.iter().any(|l| l.contains("needed uvcvideo quirk 0x100")), "{lines:?}");
    // interfaces that never come: gives up after CONFIGURE_LIMIT without touching anything
    let g = Fake::loaded();
    g.plug("1-9", 0x1d6c, 0x0103, &[]);
    g.events.borrow_mut().clear();
    assert!(handle(&g, "1-9").is_empty());
    assert_eq!(g.waited(), CONFIGURE_LIMIT);
    assert!(g.writes().iter().all(|w| w.contains("power/control")), "{:?}", g.writes());
}

#[test]
fn modules_already_loaded_are_not_loaded_again_and_a_missing_parameter_is_an_error() {
    let f = Fake::loaded();
    assert!(load_uvcvideo(&f, &f).is_ok());
    assert!(load_snd_usb_audio(&f, &f).is_ok());
    assert!(f.events().is_empty());
    let rec = crate::run::Recorder::default();
    let g = Fake::new();
    assert!(load_uvcvideo(&g, &rec).unwrap_err().contains("not loaded"));
    assert_eq!(rec.calls.lock().unwrap().as_slice(), ["modprobe uvcvideo"], "by name and without -b, so the blacklist does not stop it");
    // nothing at boot needs either driver: nothing is loaded
    let h = Fake::new();
    h.plug("1-5", 0x05ac, 0x0250, &[(0, 0x03, 1)]);
    assert!(prepare_boot(&h, &h).is_empty());
    assert!(h.events().is_empty());
}

#[test]
fn the_real_sysfs_reads_and_writes_under_its_root() {
    let root = std::env::temp_dir().join(format!("iyakki-camquirk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("bus/usb/devices/1-8:1.0")).unwrap();
    std::fs::create_dir_all(root.join("bus/usb/devices/1-8")).unwrap();
    std::fs::write(root.join("bus/usb/devices/1-8/idVendor"), "1d6c\n").unwrap();
    std::fs::write(root.join("bus/usb/devices/1-8/idProduct"), "0103\n").unwrap();
    std::fs::write(root.join("bus/usb/devices/1-8:1.0/bInterfaceClass"), "0e\n").unwrap();
    std::fs::write(root.join("bus/usb/devices/1-8:1.0/bInterfaceSubClass"), "01\n").unwrap();
    let s = RealSysfs { root: root.clone() };
    assert_eq!(ids(&s, "1-8"), Some((0x1d6c, 0x0103)));
    assert_eq!(interfaces(&s, "1-8"), vec![Interface { name: "1-8:1.0".into(), class: 0x0e, subclass: 1 }]);
    assert_eq!(usb_devices(&s), vec!["1-8"]);
    assert_eq!(known_present(&s).len(), 1);
    assert!(s.write("bus/usb/devices/1-8:1.0/authorized", "0").is_ok());
    assert_eq!(s.read("bus/usb/devices/1-8:1.0/authorized").as_deref(), Some("0"));
    assert!(s.write("nope/x", "1").is_err());
    assert!(!uvcvideo_loaded(&s));
    assert_eq!(RealSysfs::system().root, PathBuf::from("/sys"));
    let _ = std::fs::remove_dir_all(&root);
}
